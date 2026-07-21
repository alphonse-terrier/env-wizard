//! YAML config template support.
//!
//! `yaml-rust2` gives no access to comments and no format-preserving DOM, so
//! this module takes a narrower, deliberately conservative approach: use the
//! parser's event stream (with source `Marker`s) purely to discover the
//! *structure* (nested mapping paths, scalar types, line numbers), then edit
//! the document as **text**, replacing only the exact byte span of a changed
//! value on its own line. Everything else — comments, indentation, key
//! order, blank lines — is untouched because it's never read into a tree we
//! re-serialize.
//!
//! A field is only offered (and thus only ever edited) when it passes every
//! one of these checks, so if it's offered, editing it is safe:
//! - it's a scalar (string/int/float/bool/null), not an array or an anchor
//!   definition, alias, or merge key (`<<`) — none of those are supported
//!   in v1;
//! - it's written in plain, single-quoted, or double-quoted style — not a
//!   block scalar (`|`/`>`), which spans multiple lines;
//! - its key and value sit on the same physical line;
//! - a conservative re-scan of that line finds a `key: value` shape whose
//!   key matches what the parser reported — if a line looks more
//!   complicated than that (e.g. a flow mapping `{a: 1, b: 2}` sharing a
//!   line with other entries), the field is silently dropped rather than
//!   risking a bad edit.
//!
//! Only the first YAML document is read; anything after a `---` separator
//! is ignored, matching the parser's `multi = false` mode.

use anyhow::{bail, Context, Result};
use yaml_rust2::parser::{Event, MarkedEventReceiver, Parser};
use yaml_rust2::scanner::{Marker, TScalarStyle};

use super::{classify, ConfigDoc, Field, ScalarType};

struct LeafMeta {
    path: Vec<String>,
    /// 0-indexed line number of this leaf's `key: value` line.
    line: usize,
    /// Decoded original value, as the parser interpreted it (stable across edits).
    original: String,
    scalar_type: ScalarType,
    /// True if the original scalar was written in quoted style (single or
    /// double). A quoted scalar is a string *by construction*, regardless of
    /// what its text looks like — `port: "5432"` is the string `"5432"`, not
    /// the integer 5432 — so this overrides `scalar_type` when deciding how
    /// to coerce a replacement value, and keeps a replacement quoted so an
    /// edited value doesn't silently drift from string to number/bool.
    was_quoted: bool,
}

pub struct YamlDoc {
    content: String,
    leaves: Vec<LeafMeta>,
    root_is_mapping: bool,
}

impl YamlDoc {
    pub fn parse(content: &str) -> Result<Self> {
        let events = collect_events(content)?;
        let mut leaves = Vec::new();
        let mut pos = 0usize;

        let (ev, _) = next(&events, &mut pos);
        if !matches!(ev, Event::StreamStart) {
            bail!("unexpected YAML event stream: missing stream start");
        }
        let (ev, _) = next(&events, &mut pos);
        if matches!(ev, Event::StreamEnd) {
            // Empty document: no fields.
            return Ok(Self {
                content: content.to_string(),
                leaves,
                root_is_mapping: false,
            });
        }
        if !matches!(ev, Event::DocumentStart) {
            bail!("unexpected YAML event stream: missing document start");
        }

        let (root_ev, _root_mark) = next(&events, &mut pos);
        let root_is_mapping = matches!(root_ev, Event::MappingStart(..));
        if root_is_mapping {
            walk_mapping(&events, &mut pos, &[], content, &mut leaves);
        }
        // A non-mapping root (bare scalar or sequence): no fields offered.

        Ok(Self {
            content: content.to_string(),
            leaves,
            root_is_mapping,
        })
    }

    /// True if the parsed document's root is a mapping (`key: value` at the
    /// top level) rather than a bare scalar or a top-level sequence. Used by
    /// content-based format sniffing: YAML happily parses almost any text
    /// (including a single word or a dotenv `KEY=value` line) as a plain
    /// scalar, so a successful parse alone doesn't mean "this is YAML" —
    /// requiring a mapping root does.
    pub fn root_is_mapping(&self) -> bool {
        self.root_is_mapping
    }
}

impl ConfigDoc for YamlDoc {
    fn fields(&self) -> Vec<Field> {
        // A mapping may declare the same key twice (YAML's raw event stream
        // reports both; only the *last* one wins semantically, but nothing
        // here re-implements that merge). `set()` addresses a leaf by its
        // dotted path and, like a plain lookup, can only ever resolve one of
        // several identically-pathed leaves — so editing "the second one"
        // would silently land on the first. Skip every leaf whose path isn't
        // unique in the document rather than risk a wrong-line edit.
        let mut path_counts: std::collections::HashMap<&Vec<String>, usize> =
            std::collections::HashMap::new();
        for leaf in &self.leaves {
            *path_counts.entry(&leaf.path).or_insert(0) += 1;
        }

        self.leaves
            .iter()
            .filter(|leaf| path_counts.get(&leaf.path).copied().unwrap_or(0) <= 1)
            .map(|leaf| {
                let mut description = leading_comment(&self.content, leaf.line);
                if let Some(line) = self.content.lines().nth(leaf.line) {
                    let key = leaf.path.last().cloned().unwrap_or_default();
                    if let Some(parts) = parse_key_value_line(line, &key) {
                        if let Some(trailing) = parts.trailing_comment {
                            if description.is_empty() {
                                description = trailing;
                            } else {
                                description.push('\n');
                                description.push_str(&trailing);
                            }
                        }
                    }
                }
                Field {
                    display: leaf.path.join("."),
                    path: leaf.path.clone(),
                    original: leaf.original.clone(),
                    description,
                }
            })
            .collect()
    }

    fn set(&mut self, path: &[String], value: &str) -> Result<()> {
        let (line0, scalar_type, was_quoted, key_text) = {
            let leaf = self
                .leaves
                .iter()
                .find(|l| l.path.as_slice() == path)
                .with_context(|| format!("key not found while writing back: {}", path.join(".")))?;
            (
                leaf.line,
                leaf.scalar_type,
                leaf.was_quoted,
                leaf.path.last().cloned().unwrap_or_default(),
            )
        };

        let (line_start, line_end) = line_byte_span(&self.content, line0)
            .with_context(|| format!("line {} no longer exists", line0 + 1))?;
        let line_text = self.content[line_start..line_end].to_string();

        let parts = parse_key_value_line(&line_text, &key_text)
            .context("failed to safely re-locate the value on write-back")?;

        let mut new_value_text = coerce_yaml(scalar_type, was_quoted, value);
        // An implicit-null `key:` (no value at all) has an empty value span
        // sitting immediately after the colon, with no separating space.
        // Inserting text there unchanged would produce `key:value`, which
        // YAML re-reads as the plain scalar "key:value" rather than a
        // `key: value` mapping entry — so restore the separator first.
        if parts.value_start == parts.value_end
            && line_text.as_bytes().get(parts.value_start.wrapping_sub(1)) == Some(&b':')
        {
            new_value_text.insert(0, ' ');
        }
        let abs_start = line_start + parts.value_start;
        let abs_end = line_start + parts.value_end;
        self.content
            .replace_range(abs_start..abs_end, &new_value_text);
        Ok(())
    }

    fn render(&self) -> String {
        self.content.clone()
    }
}

// --- Event-stream walking -----------------------------------------------

struct EventSink {
    events: Vec<(Event, Marker)>,
}

impl MarkedEventReceiver for EventSink {
    fn on_event(&mut self, ev: Event, mark: Marker) {
        self.events.push((ev, mark));
    }
}

fn collect_events(content: &str) -> Result<Vec<(Event, Marker)>> {
    let mut parser = Parser::new_from_str(content);
    let mut sink = EventSink { events: Vec::new() };
    // `multi = false`: stop after the first document, so anything after a
    // `---` separator is simply never visited.
    parser
        .load(&mut sink, false)
        .context("failed to parse YAML")?;
    Ok(sink.events)
}

fn next<'a>(events: &'a [(Event, Marker)], pos: &mut usize) -> (&'a Event, Marker) {
    let (ev, mark) = &events[*pos];
    *pos += 1;
    (ev, *mark)
}

/// Consumes the remaining nested events of a container value (a `Sequence`
/// or `Mapping` that has already had its `*Start` event consumed as `first`)
/// without recording anything. A no-op for scalars/aliases.
fn skip_value(events: &[(Event, Marker)], pos: &mut usize, first: &Event) {
    if matches!(first, Event::SequenceStart(..) | Event::MappingStart(..)) {
        let mut depth = 1;
        while depth > 0 {
            let (ev, _) = next(events, pos);
            match ev {
                Event::SequenceStart(..) | Event::MappingStart(..) => depth += 1,
                Event::SequenceEnd | Event::MappingEnd => depth -= 1,
                _ => {}
            }
        }
    }
}

fn walk_mapping(
    events: &[(Event, Marker)],
    pos: &mut usize,
    path: &[String],
    content: &str,
    leaves: &mut Vec<LeafMeta>,
) {
    loop {
        let (key_ev, key_mark) = next(events, pos);
        if matches!(key_ev, Event::MappingEnd) {
            return;
        }

        let key_scalar = match key_ev {
            Event::Scalar(value, _style, anchor_id, _tag) => Some((value.clone(), *anchor_id)),
            _ => None,
        };

        let Some((key_text, key_anchor)) = key_scalar else {
            // Complex (non-scalar) key: not supported, skip key + value.
            skip_value(events, pos, key_ev);
            let (val_ev, _) = next(events, pos);
            skip_value(events, pos, val_ev);
            continue;
        };

        let (val_ev, val_mark) = next(events, pos);

        if key_text == "<<" {
            // Merge key: not supported, skip its value untouched.
            skip_value(events, pos, val_ev);
            continue;
        }

        let mut child_path = path.to_vec();
        child_path.push(key_text.clone());

        match val_ev {
            Event::MappingStart(..) => {
                walk_mapping(events, pos, &child_path, content, leaves);
            }
            Event::SequenceStart(..) => {
                // Arrays: not offered in v1.
                skip_value(events, pos, val_ev);
            }
            Event::Scalar(value, style, val_anchor, _tag) => {
                let editable = key_anchor == 0
                    && *val_anchor == 0
                    && matches!(
                        style,
                        TScalarStyle::Plain
                            | TScalarStyle::SingleQuoted
                            | TScalarStyle::DoubleQuoted
                    )
                    && key_mark.line() == val_mark.line();

                if editable {
                    let line0 = val_mark.line().saturating_sub(1);
                    if let Some(line_text) = content.lines().nth(line0) {
                        if parse_key_value_line(line_text, &key_text).is_some() {
                            let was_quoted = !matches!(style, TScalarStyle::Plain);
                            // A quoted scalar is a string regardless of what
                            // its text looks like; only classify plain
                            // (unquoted) scalars by content.
                            let scalar_type = if was_quoted {
                                ScalarType::String
                            } else {
                                classify(value)
                            };
                            leaves.push(LeafMeta {
                                path: child_path,
                                line: line0,
                                original: value.clone(),
                                scalar_type,
                                was_quoted,
                            });
                        }
                    }
                }
            }
            Event::Alias(_) => {
                // Aliased value: not offered in v1.
            }
            _ => {}
        }
    }
}

// --- Raw-line surgery ----------------------------------------------------

struct LineParts {
    /// Byte range of the value token within the line (comment/trailing
    /// whitespace excluded).
    value_start: usize,
    value_end: usize,
    trailing_comment: Option<String>,
}

/// Parses one physical line believed to be `<indent><key>: <value>[ #comment]`.
/// Returns `None` if the line doesn't match that simple shape for
/// `expected_key` (e.g. a flow collection sharing the line with siblings) —
/// callers must then treat the field as unsafe to touch.
fn parse_key_value_line(line: &str, expected_key: &str) -> Option<LineParts> {
    let colon = find_unquoted_colon(line)?;
    let key_part = line[..colon].trim();
    if dequote_simple(key_part) != expected_key {
        return None;
    }

    let after_colon = &line[colon + 1..];
    let ws_len = after_colon.len() - after_colon.trim_start().len();
    let value_start = colon + 1 + ws_len;
    let rest = &line[value_start..];

    let comment_rel = find_value_end(rest);
    let value_text = rest[..comment_rel].trim_end();
    if value_text.starts_with('[') || value_text.starts_with('{') {
        // Flow collection: not a scalar leaf we support in v1.
        return None;
    }
    let value_end = value_start + value_text.len();

    let trailing_comment = if comment_rel < rest.len() {
        let stripped = rest[comment_rel..].trim().trim_start_matches('#').trim();
        (!stripped.is_empty()).then(|| stripped.to_string())
    } else {
        None
    };

    Some(LineParts {
        value_start,
        value_end,
        trailing_comment,
    })
}

/// Finds the byte offset of the first `:` outside quotes that acts as a
/// mapping key/value separator (followed by whitespace or end of line).
fn find_unquoted_colon(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    for (i, &c) in bytes.iter().enumerate() {
        match c {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b':' if !in_single && !in_double => {
                let next = bytes.get(i + 1).copied();
                if next.is_none() || next == Some(b' ') || next == Some(b'\t') {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Byte length of `s` up to (not including) a comment start: a `#` outside
/// quotes that is at the start of `s` or preceded by whitespace. Returns
/// `s.len()` if there's no comment.
fn find_value_end(s: &str) -> usize {
    let bytes = s.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    for (i, &c) in bytes.iter().enumerate() {
        match c {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'#' if !in_single
                && !in_double
                && (i == 0 || bytes[i - 1] == b' ' || bytes[i - 1] == b'\t') =>
            {
                return i;
            }
            _ => {}
        }
    }
    bytes.len()
}

/// Best-effort unquoting for comparing an on-disk key to the parser's
/// decoded key. Bails out (returns something that won't match) on anything
/// containing backslash escapes, rather than risk a wrong match.
fn dequote_simple(s: &str) -> String {
    if s.len() >= 2 {
        let bytes = s.as_bytes();
        if bytes[0] == b'"' && bytes[s.len() - 1] == b'"' {
            let inner = &s[1..s.len() - 1];
            if inner.contains('\\') {
                return format!("\u{0}unsupported-escape:{inner}");
            }
            return inner.to_string();
        }
        if bytes[0] == b'\'' && bytes[s.len() - 1] == b'\'' {
            return s[1..s.len() - 1].replace("''", "'");
        }
    }
    s.to_string()
}

/// Byte range of line `line0` (0-indexed) within `content`, excluding its
/// line terminator (`\n` or `\r\n`).
fn line_byte_span(content: &str, line0: usize) -> Option<(usize, usize)> {
    let mut offset = 0;
    for (idx, line) in content.split_inclusive('\n').enumerate() {
        if idx == line0 {
            let mut end = offset + line.len();
            if let Some(stripped) = line.strip_suffix('\n') {
                end -= 1;
                if stripped.ends_with('\r') {
                    end -= 1;
                }
            }
            return Some((offset, end));
        }
        offset += line.len();
    }
    None
}

/// Leading standalone `#` comment lines directly above `line0`, closest
/// first then reversed into document order.
fn leading_comment(content: &str, line0: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let mut collected = Vec::new();
    let mut i = line0;
    while i > 0 {
        i -= 1;
        let trimmed = lines[i].trim();
        match trimmed.strip_prefix('#') {
            Some(comment) => collected.push(comment.trim().to_string()),
            None => break,
        }
    }
    collected.reverse();
    collected.join("\n")
}

// --- Type-preserving coercion / quoting ---------------------------------

const YAML_RESERVED_WORDS: &[&str] = &["true", "false", "yes", "no", "on", "off", "null", "~"];

/// True if writing `s` as a bare (unquoted) YAML plain scalar would risk it
/// being re-read as something other than a plain string.
fn needs_yaml_quoting(s: &str) -> bool {
    if s.is_empty() || s.trim() != s {
        return true;
    }
    if YAML_RESERVED_WORDS.contains(&s.to_ascii_lowercase().as_str()) {
        return true;
    }
    if s.parse::<i64>().is_ok() || s.parse::<f64>().is_ok() {
        return true;
    }
    if s.contains(": ") || s.contains(" #") || s.ends_with(':') {
        return true;
    }
    let first = s.chars().next().expect("checked non-empty above");
    "-?:,[]{}#&*!|>'\"%@`".contains(first)
}

fn yaml_quote_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Coerces `input` to text matching `old_type`, falling back to a quoted
/// string if `input` doesn't parse as that type.
///
/// `was_quoted` is true when the field's *original* value was written in
/// quoted style. Such a field is always `ScalarType::String` (see
/// [`walk_mapping`]'s use of it), so this only affects the `String` arm
/// below — but it matters: without it, a numeric- or bool-looking
/// replacement for an originally-quoted string (`port: "5432"` -> `5433`)
/// would render bare (`port: 5433`), silently turning a string into a number
/// on re-parse. Once a field was quoted, replacements stay quoted too, so
/// the field's type never drifts across an edit.
fn coerce_yaml(old_type: ScalarType, was_quoted: bool, input: &str) -> String {
    let matches_old_type = match old_type {
        ScalarType::Int => input.parse::<i64>().is_ok(),
        ScalarType::Float => input.parse::<f64>().is_ok(),
        ScalarType::Bool => input == "true" || input == "false",
        ScalarType::Null => input.is_empty() || input.eq_ignore_ascii_case("null") || input == "~",
        ScalarType::String => true,
    };

    if !matches_old_type {
        return yaml_quote_string(input);
    }

    match old_type {
        ScalarType::Null => "null".to_string(),
        ScalarType::String if was_quoted || needs_yaml_quoting(input) => yaml_quote_string(input),
        _ => input.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::apply_answers;

    const EXAMPLE: &str = "\
# Database connection
database:
  # Hostname to connect to
  host: localhost
  port: 5432 # default port
  enabled: true

cache:
  ttl: 60
";

    #[test]
    fn fields_have_dotted_paths_defaults_and_comments() {
        let doc = YamlDoc::parse(EXAMPLE).unwrap();
        let fields = doc.fields();

        let host = fields
            .iter()
            .find(|f| f.display == "database.host")
            .unwrap();
        assert_eq!(host.original, "localhost");
        assert_eq!(host.description, "Hostname to connect to");
        assert_eq!(host.path, vec!["database".to_string(), "host".to_string()]);

        let port = fields
            .iter()
            .find(|f| f.display == "database.port")
            .unwrap();
        assert_eq!(port.original, "5432");
        assert_eq!(port.description, "default port");

        let ttl = fields.iter().find(|f| f.display == "cache.ttl").unwrap();
        assert_eq!(ttl.original, "60");
    }

    #[test]
    fn round_trip_identity_when_nothing_changes() {
        let mut doc = YamlDoc::parse(EXAMPLE).unwrap();
        let fields = doc.fields();
        let answers: Vec<String> = fields.iter().map(|f| f.original.clone()).collect();
        apply_answers(&mut doc, &fields, &answers).unwrap();
        assert_eq!(doc.render(), EXAMPLE);
    }

    #[test]
    fn changed_value_keeps_type_and_trailing_comment() {
        let mut doc = YamlDoc::parse(EXAMPLE).unwrap();
        doc.set(&["database".to_string(), "port".to_string()], "6543")
            .unwrap();
        let out = doc.render();
        assert!(out.contains("port: 6543 # default port"));
        assert!(!out.contains("port: \"6543\""));
    }

    #[test]
    fn changed_string_value_stays_bare_when_safe() {
        let mut doc = YamlDoc::parse(EXAMPLE).unwrap();
        doc.set(&["database".to_string(), "host".to_string()], "db.prod")
            .unwrap();
        let out = doc.render();
        assert!(out.contains("host: db.prod"));
    }

    #[test]
    fn changed_string_value_is_quoted_when_ambiguous() {
        let mut doc = YamlDoc::parse(EXAMPLE).unwrap();
        doc.set(&["database".to_string(), "host".to_string()], "yes")
            .unwrap();
        let out = doc.render();
        assert!(out.contains("host: \"yes\""));
    }

    #[test]
    fn merge_keys_and_block_scalars_are_not_offered() {
        let yaml = "\
defaults: &defaults
  timeout: 30
service:
  <<: *defaults
  notes: |
    multi
    line
  name: svc
";
        let doc = YamlDoc::parse(yaml).unwrap();
        let fields = doc.fields();
        // The merge key itself is never a real field.
        assert!(fields.iter().all(|f| f.display != "service.<<"));
        // Block (literal/folded) scalars span multiple lines: not offered in v1.
        assert!(fields.iter().all(|f| f.display != "service.notes"));
        // Ordinary scalars remain offered, even under an anchor-defining
        // mapping: editing them safely updates the anchor, which any alias
        // site picks up when re-parsed.
        assert!(fields.iter().any(|f| f.display == "defaults.timeout"));
        assert!(fields.iter().any(|f| f.display == "service.name"));
    }

    #[test]
    fn anchored_scalar_value_is_not_offered() {
        let yaml = "\
timeout: &default_timeout 30
retry_timeout: *default_timeout
";
        let doc = YamlDoc::parse(yaml).unwrap();
        let fields = doc.fields();
        assert!(fields.iter().all(|f| f.display != "timeout"));
        assert!(fields.iter().all(|f| f.display != "retry_timeout"));
    }

    #[test]
    fn duplicate_key_is_not_offered_but_siblings_still_are() {
        let yaml = "\
host: first
host: second
port: 5432
";
        let doc = YamlDoc::parse(yaml).unwrap();
        let fields = doc.fields();
        assert!(fields.iter().all(|f| f.display != "host"));
        assert!(fields.iter().any(|f| f.display == "port"));
    }

    #[test]
    fn duplicate_key_lines_are_untouched_when_editing_a_sibling() {
        let yaml = "\
host: first
host: second
port: 5432
";
        let mut doc = YamlDoc::parse(yaml).unwrap();
        let fields = doc.fields();
        let answers: Vec<String> = fields
            .iter()
            .map(|f| {
                if f.display == "port" {
                    "6543".to_string()
                } else {
                    f.original.clone()
                }
            })
            .collect();
        apply_answers(&mut doc, &fields, &answers).unwrap();
        let out = doc.render();
        assert_eq!(out, "host: first\nhost: second\nport: 6543\n");
    }

    #[test]
    fn quoted_numeric_string_keeps_its_type_and_quotes_when_edited() {
        // `port` was authored as an explicitly-quoted string, even though its
        // text looks numeric. Editing it must not silently turn it into an
        // int and drop the quotes.
        let yaml = "port: \"5432\"\n";
        let mut doc = YamlDoc::parse(yaml).unwrap();
        let fields = doc.fields();
        let port = fields.iter().find(|f| f.display == "port").unwrap();
        assert_eq!(port.original, "5432");

        doc.set(&["port".to_string()], "5433").unwrap();
        let out = doc.render();
        assert_eq!(out, "port: \"5433\"\n");
    }

    #[test]
    fn quoted_bool_and_null_looking_strings_keep_their_type_and_quotes() {
        let yaml = "enabled: \"true\"\nvalue: \"null\"\n";
        let mut doc = YamlDoc::parse(yaml).unwrap();

        doc.set(&["enabled".to_string()], "false").unwrap();
        doc.set(&["value".to_string()], "something").unwrap();
        let out = doc.render();
        assert!(out.contains("enabled: \"false\""));
        assert!(out.contains("value: \"something\""));
    }

    #[test]
    fn unquoted_numeric_value_is_unaffected_by_the_quote_style_fix() {
        // A plain (unquoted) numeric scalar must still classify and coerce as
        // before: bare in, bare out.
        let mut doc = YamlDoc::parse(EXAMPLE).unwrap();
        doc.set(&["database".to_string(), "port".to_string()], "6543")
            .unwrap();
        let out = doc.render();
        assert!(out.contains("port: 6543 # default port"));
    }

    #[test]
    fn filling_an_empty_value_inserts_a_separating_space() {
        let yaml = "host:\nport: 5432\n";
        let mut doc = YamlDoc::parse(yaml).unwrap();
        let fields = doc.fields();
        let host = fields.iter().find(|f| f.display == "host");
        let Some(host) = host else {
            // If yaml-rust2 doesn't offer an implicit-null leaf as editable,
            // there's nothing to fix — document that and stop here.
            return;
        };
        assert_eq!(host.original, "");

        doc.set(&["host".to_string()], "localhost").unwrap();
        let out = doc.render();
        // Must produce a real `key: value` mapping entry, not `key:value`
        // (which YAML would re-read as the single plain scalar "key:value").
        assert!(out.contains("host: localhost"));
        assert!(!out.contains("host:localhost"));

        let reparsed = YamlDoc::parse(&out).unwrap();
        let reparsed_host = reparsed
            .fields()
            .into_iter()
            .find(|f| f.display == "host")
            .expect("host should still be a mapping entry after the edit");
        assert_eq!(reparsed_host.original, "localhost");
    }
}
