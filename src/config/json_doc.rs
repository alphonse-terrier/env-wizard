//! JSON / JSONC config template support, backed by `jsonc-parser`'s
//! format-preserving CST. `//` and `/* ... */` comments (JSONC) are read as
//! field descriptions.
//!
//! Only scalar leaves (string/number/bool/null) are exposed as [`Field`]s.
//! Arrays are left untouched in v1 (not offered to the user); nested objects
//! are recursed into for dotted paths.

use anyhow::{bail, Context, Result};
use jsonc_parser::cst::{
    CstInputValue, CstLeafNode, CstNode, CstObject, CstObjectProp, CstRootNode,
};
use jsonc_parser::ParseOptions;

use super::{ConfigDoc, Field, ScalarType};

pub struct JsonDoc {
    root: CstRootNode,
}

impl JsonDoc {
    pub fn parse(content: &str) -> Result<Self> {
        let root = CstRootNode::parse(content, &ParseOptions::default())
            .context("failed to parse JSON")?;
        Ok(Self { root })
    }

    /// True if the document's root value is a JSON object. Used by
    /// content-based format sniffing: a bare JSON scalar or a top-level array
    /// isn't the shape env-wizard prompts over, so it shouldn't count as a
    /// decisive JSON sniff.
    pub fn has_object_root(&self) -> bool {
        self.root.object_value().is_some()
    }
}

impl ConfigDoc for JsonDoc {
    fn fields(&self) -> Vec<Field> {
        let mut out = Vec::new();
        if let Some(obj) = self.root.object_value() {
            collect_fields(&obj, &[], &mut out);
        }
        out
    }

    fn set(&mut self, path: &[String], value: &str) -> Result<()> {
        let obj = self
            .root
            .object_value()
            .context("document root is not a JSON object")?;
        set_path(&obj, path, value)
    }

    fn render(&self) -> String {
        self.root.to_string()
    }
}

fn collect_fields(obj: &CstObject, prefix: &[String], out: &mut Vec<Field>) {
    // Property names duplicated at this level (JSON permits it, unlike TOML).
    // `set()` addresses a field by its dotted path and — like `obj.get()` —
    // can only ever resolve the *first* occurrence, so editing the second
    // would silently land on the first instead. Skip every occurrence of a
    // duplicated key entirely (including recursing into it) rather than risk
    // a wrong-span edit.
    let mut name_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for prop in obj.properties() {
        if let Some(name) = prop.name().and_then(|n| n.decoded_value().ok()) {
            *name_counts.entry(name).or_insert(0) += 1;
        }
    }

    for prop in obj.properties() {
        let Some(name) = prop.name().and_then(|n| n.decoded_value().ok()) else {
            continue;
        };
        if name_counts.get(&name).copied().unwrap_or(0) > 1 {
            continue;
        }
        let mut path = prefix.to_vec();
        path.push(name);

        let Some(value_node) = prop.value() else {
            continue;
        };

        if let Some(sub) = value_node.as_object() {
            collect_fields(&sub, &path, out);
            continue;
        }

        let Some(scalar_type) = scalar_type_of(&value_node) else {
            // Arrays: not offered in v1.
            continue;
        };
        let Some(original) = render_scalar(&value_node, scalar_type) else {
            continue;
        };

        out.push(Field {
            display: path.join("."),
            path,
            original,
            description: leading_comment(&prop),
        });
    }
}

fn scalar_type_of(node: &CstNode) -> Option<ScalarType> {
    if node.as_string_lit().is_some() {
        return Some(ScalarType::String);
    }
    if let Some(n) = node.as_number_lit() {
        return Some(if n.to_string().parse::<i64>().is_ok() {
            ScalarType::Int
        } else {
            ScalarType::Float
        });
    }
    if node.as_boolean_lit().is_some() {
        return Some(ScalarType::Bool);
    }
    if node.as_null_keyword().is_some() {
        return Some(ScalarType::Null);
    }
    None
}

fn render_scalar(node: &CstNode, scalar_type: ScalarType) -> Option<String> {
    match scalar_type {
        ScalarType::String => node.as_string_lit()?.decoded_value().ok(),
        ScalarType::Int | ScalarType::Float => Some(node.as_number_lit()?.to_string()),
        ScalarType::Bool => Some(node.as_boolean_lit()?.value().to_string()),
        ScalarType::Null => Some("null".to_string()),
    }
}

/// Leading `//` / `/* ... */` comment lines directly above this property,
/// walking back through whitespace/newlines/the previous trailing comma.
fn leading_comment(prop: &CstObjectProp) -> String {
    let node: CstNode = prop.clone().into();
    let mut lines = Vec::new();

    for sibling in node.previous_siblings() {
        match sibling {
            CstNode::Leaf(CstLeafNode::Comment(comment)) => {
                lines.push(strip_comment_marker(&comment.raw_value()));
            }
            CstNode::Leaf(CstLeafNode::Whitespace(_)) | CstNode::Leaf(CstLeafNode::Newline(_)) => {
                continue;
            }
            CstNode::Leaf(CstLeafNode::Token(token)) if token.value() == ',' => continue,
            _ => break,
        }
    }

    lines.reverse();
    lines.join("\n")
}

fn strip_comment_marker(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix("//") {
        rest.trim().to_string()
    } else if let Some(rest) = trimmed.strip_prefix("/*") {
        rest.trim_end_matches("*/").trim().to_string()
    } else {
        trimmed.to_string()
    }
}

fn set_path(obj: &CstObject, path: &[String], value: &str) -> Result<()> {
    match path {
        [] => bail!("cannot set an empty path"),
        [last] => {
            let prop = obj
                .get(last)
                .with_context(|| format!("key not found while writing back: {last}"))?;
            set_scalar(&prop, value)
        }
        [head, rest @ ..] => {
            let prop = obj
                .get(head)
                .with_context(|| format!("key not found while writing back: {head}"))?;
            let sub = prop
                .value()
                .and_then(|v| v.as_object())
                .with_context(|| format!("{head} is not an object"))?;
            set_path(&sub, rest, value)
        }
    }
}

fn set_scalar(prop: &CstObjectProp, input: &str) -> Result<()> {
    let value_node = prop.value().context("property has no value")?;
    let old_type = scalar_type_of(&value_node).context("target's type isn't editable in v1")?;
    prop.set_value(coerce(old_type, input));
    Ok(())
}

/// True if `input` is a syntactically valid JSON number literal (RFC 8259):
/// unlike Rust's `str::parse::<i64>()`/`parse::<f64>()`, JSON rejects a
/// leading `+` (`+5`), leading zeros (`007`), a fraction with no digits on
/// either side (`.5`, `1.`), and the non-finite tokens `inf`/`nan`/`infinity`
/// — all of which Rust's parsers happily accept. Delegates to `serde_json`
/// (already a dependency) rather than hand-rolling the grammar.
fn is_json_number(input: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(input)
        .map(|v| v.is_number())
        .unwrap_or(false)
}

/// Coerces `input` to a `CstInputValue` matching `old_type`, falling back to
/// a JSON string if it doesn't parse as that type. The fallback matters here
/// more than in the TOML/YAML siblings: `jsonc_parser::CstObjectProp::set_value`
/// performs no validation, so writing anything other than a genuine JSON
/// number token through the `Number` variant would silently corrupt the file
/// with unparseable output.
fn coerce(old_type: ScalarType, input: &str) -> CstInputValue {
    match old_type {
        ScalarType::Int | ScalarType::Float => {
            if is_json_number(input) {
                CstInputValue::Number(input.to_string())
            } else {
                CstInputValue::String(input.to_string())
            }
        }
        ScalarType::Bool => match input {
            "true" => CstInputValue::Bool(true),
            "false" => CstInputValue::Bool(false),
            _ => CstInputValue::String(input.to_string()),
        },
        ScalarType::Null => {
            if input.is_empty() || input == "null" {
                CstInputValue::Null
            } else {
                CstInputValue::String(input.to_string())
            }
        }
        ScalarType::String => CstInputValue::String(input.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::apply_answers;

    const EXAMPLE: &str = r#"{
  // Database connection
  "database": {
    // Hostname to connect to
    "host": "localhost",
    "port": 5432,
    "enabled": true
  },
  "cache": {
    "ttl": 60
  }
}"#;

    #[test]
    fn fields_have_dotted_paths_defaults_and_comments() {
        let doc = JsonDoc::parse(EXAMPLE).unwrap();
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

        let ttl = fields.iter().find(|f| f.display == "cache.ttl").unwrap();
        assert_eq!(ttl.original, "60");
    }

    #[test]
    fn round_trip_identity_when_nothing_changes() {
        let mut doc = JsonDoc::parse(EXAMPLE).unwrap();
        let fields = doc.fields();
        let answers: Vec<String> = fields.iter().map(|f| f.original.clone()).collect();
        apply_answers(&mut doc, &fields, &answers).unwrap();
        assert_eq!(doc.render(), EXAMPLE);
    }

    #[test]
    fn changed_value_keeps_type() {
        let mut doc = JsonDoc::parse(EXAMPLE).unwrap();
        doc.set(&["database".to_string(), "port".to_string()], "6543")
            .unwrap();
        let out = doc.render();
        assert!(out.contains("\"port\": 6543"));
        assert!(!out.contains("\"port\": \"6543\""));
    }

    #[test]
    fn changed_string_value_is_quoted_and_escaped() {
        let mut doc = JsonDoc::parse(EXAMPLE).unwrap();
        doc.set(&["database".to_string(), "host".to_string()], "db.prod")
            .unwrap();
        let out = doc.render();
        assert!(out.contains("\"host\": \"db.prod\""));
    }

    #[test]
    fn changed_numeric_field_still_renders_bare_for_valid_numbers() {
        let mut doc = JsonDoc::parse(EXAMPLE).unwrap();
        doc.set(&["database".to_string(), "port".to_string()], "-1.5e3")
            .unwrap();
        let out = doc.render();
        assert!(out.contains("\"port\": -1.5e3"));
        // Sanity: still valid (JSONC — the fixture has `//` comments) JSON.
        JsonDoc::parse(&out).unwrap();
    }

    #[test]
    fn changed_numeric_field_falls_back_to_a_quoted_string_for_non_json_numbers() {
        // Each of these is accepted by Rust's `str::parse::<i64>/<f64>` but is
        // NOT a valid JSON number token — writing it bare would corrupt the
        // file. All must fall back to a quoted JSON string instead.
        for input in ["007", "+5", ".5", "1.", "inf", "nan", "infinity"] {
            let mut doc = JsonDoc::parse(EXAMPLE).unwrap();
            doc.set(&["database".to_string(), "port".to_string()], input)
                .unwrap();
            let out = doc.render();
            let expected = format!("\"port\": \"{input}\"");
            assert!(
                out.contains(&expected),
                "input {input:?}: expected {expected:?} in {out:?}"
            );
            // The whole document must still be valid, parseable JSON(C).
            JsonDoc::parse(&out)
                .unwrap_or_else(|e| panic!("input {input:?} produced invalid JSON: {e}\n{out}"));
        }
    }

    #[test]
    fn duplicate_key_is_not_offered_but_siblings_still_are() {
        let example = r#"{
  "host": "first",
  "host": "second",
  "port": 5432
}"#;
        let doc = JsonDoc::parse(example).unwrap();
        let fields = doc.fields();
        assert!(fields.iter().all(|f| f.display != "host"));
        assert!(fields.iter().any(|f| f.display == "port"));
    }

    #[test]
    fn duplicate_key_lines_are_untouched_when_editing_a_sibling() {
        let example = r#"{
  "host": "first",
  "host": "second",
  "port": 5432
}"#;
        let mut doc = JsonDoc::parse(example).unwrap();
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
        assert!(out.contains("\"host\": \"first\""));
        assert!(out.contains("\"host\": \"second\""));
        assert!(out.contains("\"port\": 6543"));
    }
}
