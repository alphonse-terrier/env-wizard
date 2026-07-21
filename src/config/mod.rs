//! Structured config-template support: read a `config.example.{toml,yaml,json}`
//! (or `.sample`/`.dist`/`.template` variant) and help the user produce the real
//! config file, mirroring how `.env.example` produces `.env`.
//!
//! Unlike dotenv, these formats are typed and hierarchical, so the parsed unit
//! is [`Field`] (a scalar leaf with a dotted display path) rather than
//! [`crate::parser::EnvVar`] directly — [`crate::main`] converts `Field` into
//! an `EnvVar` for the shared interactive prompt, then applies the answers
//! back onto the [`ConfigDoc`] positionally (never re-parsing the display
//! string), so keys that themselves contain dots are never ambiguous.
//!
//! Only scalar leaves (string/int/float/bool/null) are offered; arrays and
//! anything the per-format writer can't safely preserve are left untouched.

mod json_doc;
mod toml_doc;
mod yaml_doc;

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

/// A scalar leaf discovered in a config template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// Dotted path for display/prompting, e.g. `database.host`.
    pub display: String,
    /// Structured path used to address the value when writing back. Never
    /// re-derived from `display`, so keys containing literal dots are safe.
    pub path: Vec<String>,
    /// The value exactly as it appeared in the example, rendered as a string.
    pub original: String,
    /// Leading comment(s) documenting this field, if any.
    pub description: String,
}

/// A parsed structured config document that can enumerate its scalar leaves,
/// update one by structured path, and re-render itself preserving formatting,
/// comments, and ordering for everything that wasn't changed.
pub trait ConfigDoc {
    /// Ordered scalar leaves found in the document.
    fn fields(&self) -> Vec<Field>;

    /// Set the value at `path`, coercing `value` to the original leaf's type
    /// when possible (falling back to a string).
    fn set(&mut self, path: &[String], value: &str) -> Result<()>;

    /// Re-serialize the document. Untouched regions must be byte-identical to
    /// the source that was opened.
    fn render(&self) -> String;
}

/// The structured formats env-wizard understands, keyed off file extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Toml,
    Yaml,
    Json,
}

/// Filename segments (case-insensitive) that mark a file as a template rather
/// than the real config, e.g. `config.example.toml` or `config.toml.example`.
const TEMPLATE_MARKERS: [&str; 4] = ["example", "sample", "dist", "template"];

/// Maps a file extension (without the leading dot) to its [`Format`].
pub fn format_from_extension(ext: &str) -> Option<Format> {
    match ext.to_ascii_lowercase().as_str() {
        "toml" => Some(Format::Toml),
        "yaml" | "yml" => Some(Format::Yaml),
        "json" => Some(Format::Json),
        _ => None,
    }
}

/// True if `part` (one dot-separated filename segment) is a template marker.
fn is_template_marker(part: &str) -> bool {
    TEMPLATE_MARKERS
        .iter()
        .any(|marker| part.eq_ignore_ascii_case(marker))
}

/// True if `path`'s filename contains a template marker segment (in any
/// position, e.g. `config.example.toml` or `config.toml.example`), regardless
/// of extension. This is the filename-only half of template detection — it
/// answers "is this a template at all", not "which format". A file with no
/// marker is never a template, no matter what its content looks like: content
/// alone can't distinguish a template from the real config file it would
/// eventually produce.
pub fn has_template_marker(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    name.split('.').any(is_template_marker)
}

/// The canonical file extension for `format`, used when a detected template
/// has no extension of its own (e.g. `config.example` with no trailing
/// `.toml`/`.yaml`/`.json`).
fn canonical_extension(format: Format) -> &'static str {
    match format {
        Format::Toml => "toml",
        Format::Yaml => "yaml",
        Format::Json => "json",
    }
}

/// Sniffs `content` for a decisive structured format, trying formats from
/// most-specific to most-permissive so an ambiguous parse never wins.
///
/// Returns `Some` only when the content is unambiguously that format:
/// - **JSON** first: parses as JSONC AND its root is an object. (A bare JSON
///   value or top-level array isn't the shape env-wizard prompts over, and a
///   JSON object also happens to be valid YAML, so JSON must be tried before
///   YAML or YAML would always win.)
/// - **TOML** next: `toml_edit` parses it AND it has at least one top-level
///   key. TOML's grammar is strict — a dotenv `KEY=value` line (a bare,
///   unquoted word) fails to parse — so dotenv content is never mistaken for
///   TOML here.
/// - **YAML** last, as the most permissive format: parses AND its root is a
///   mapping. `yaml-rust2` will happily accept almost any text (a single bare
///   word, a dotenv `KEY=value` line) as a one-line plain scalar; requiring a
///   mapping *root* is what excludes those.
///
/// Returns `None` for empty/comment-only content or anything that doesn't
/// decisively match one of the three shapes above — callers should fall back
/// to the filename extension in that case.
pub fn detect_format_from_content(content: &str) -> Option<Format> {
    if json_doc::JsonDoc::parse(content)
        .map(|doc| doc.has_object_root())
        .unwrap_or(false)
    {
        return Some(Format::Json);
    }
    if toml_doc::TomlDoc::parse(content)
        .map(|doc| !doc.is_empty())
        .unwrap_or(false)
    {
        return Some(Format::Toml);
    }
    if yaml_doc::YamlDoc::parse(content)
        .map(|doc| doc.root_is_mapping())
        .unwrap_or(false)
    {
        return Some(Format::Yaml);
    }
    None
}

/// Resolves the format of a structured config template, given both its path
/// and its content. Returns `None` if `path` isn't marked as a template at
/// all (see [`has_template_marker`]) — content is never consulted in that
/// case, since a marker-less file is either a real config (must not be
/// touched) or a dotenv example (handled separately).
///
/// Content is authoritative: [`detect_format_from_content`] is tried first,
/// and the filename extension is only a tiebreaker used when the content
/// isn't decisively any one format (empty, comment-only, or otherwise
/// inconclusive). This lets a misnamed template (`config.example.json` that
/// actually holds TOML) or an extension-less one (`config.example`) still be
/// detected correctly.
pub fn resolve_template_format(path: &Path, content: &str) -> Option<Format> {
    if !has_template_marker(path) {
        return None;
    }
    detect_format_from_content(content).or_else(|| {
        path.file_name()
            .and_then(|n| n.to_str())
            .and_then(|name| name.split('.').rev().find_map(format_from_extension))
    })
}

/// Derives the real config filename from a template filename by dropping the
/// marker segment: `config.example.toml` -> `config.toml`,
/// `settings.sample.yaml` -> `settings.yaml`, `config.toml.example` -> `config.toml`.
///
/// `format` is the format detected for this template (typically via
/// [`resolve_template_format`], which prefers content over extension).
/// If, after dropping the marker segment, the remaining extension doesn't
/// map to `format` — either because there's no extension at all
/// (`config.example` -> `config`) or because the extension actually names a
/// *different* format than the content did (a misnamed template, e.g.
/// `settings.example.yaml` whose content sniffed as JSON) — it's replaced
/// with `format`'s canonical extension, so the written file's name always
/// reflects the format that was actually used to parse and render it.
pub fn derive_output_name(path: &Path, format: Format) -> PathBuf {
    let dir = path.parent().unwrap_or_else(|| Path::new(""));
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

    let parts: Vec<&str> = name.split('.').collect();
    let kept: Vec<&str> = parts
        .into_iter()
        .filter(|p| !is_template_marker(p))
        .collect();

    let derived = if kept.is_empty() {
        name.to_string()
    } else {
        kept.join(".")
    };

    let derived_path = Path::new(&derived);
    let extension_matches_format = derived_path
        .extension()
        .and_then(|e| e.to_str())
        .and_then(format_from_extension)
        == Some(format);

    let final_name = if extension_matches_format {
        derived
    } else {
        let stem = derived_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&derived);
        format!("{stem}.{}", canonical_extension(format))
    };

    dir.join(final_name)
}

/// Parses `content` as `format` into a boxed [`ConfigDoc`].
pub fn open(format: Format, content: &str) -> Result<Box<dyn ConfigDoc>> {
    match format {
        Format::Toml => Ok(Box::new(toml_doc::TomlDoc::parse(content)?)),
        Format::Yaml => Ok(Box::new(yaml_doc::YamlDoc::parse(content)?)),
        Format::Json => Ok(Box::new(json_doc::JsonDoc::parse(content)?)),
    }
}

/// The inferred scalar type of a value token, used to decide how to coerce a
/// user-typed replacement and whether it needs quoting on write-back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarType {
    Int,
    Float,
    Bool,
    Null,
    String,
}

/// Classifies a raw (unquoted) scalar token by what it looks like.
pub fn classify(token: &str) -> ScalarType {
    if token.is_empty() {
        return ScalarType::String;
    }
    if token.parse::<i64>().is_ok() {
        return ScalarType::Int;
    }
    if token.parse::<f64>().is_ok() {
        return ScalarType::Float;
    }
    match token {
        "true" | "false" => ScalarType::Bool,
        "null" | "~" => ScalarType::Null,
        _ => ScalarType::String,
    }
}

/// Applies user answers onto `doc`, skipping any answer identical to the
/// field's original value. This — not formatting-neutral `set()` calls — is
/// what guarantees byte-identical output for anything the user left alone.
pub fn apply_answers(doc: &mut dyn ConfigDoc, fields: &[Field], answers: &[String]) -> Result<()> {
    if fields.len() != answers.len() {
        bail!(
            "field/answer count mismatch: {} fields, {} answers",
            fields.len(),
            answers.len()
        );
    }
    for (field, answer) in fields.iter().zip(answers) {
        if answer == &field.original {
            continue;
        }
        doc.set(&field.path, answer)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_template_marker_detects_marker_before_or_after_extension() {
        assert!(has_template_marker(Path::new("config.example.toml")));
        assert!(has_template_marker(Path::new("settings.sample.yaml")));
        assert!(has_template_marker(Path::new("appsettings.example.json")));
        assert!(has_template_marker(Path::new("config.toml.example")));
        assert!(has_template_marker(Path::new(".env.example")));
        assert!(has_template_marker(Path::new("readme.example.md")));
    }

    #[test]
    fn has_template_marker_rejects_plain_filenames() {
        assert!(!has_template_marker(Path::new("config.toml")));
        assert!(!has_template_marker(Path::new("appsettings.json")));
    }

    #[test]
    fn derives_output_name_marker_before_extension() {
        assert_eq!(
            derive_output_name(Path::new("config.example.toml"), Format::Toml),
            PathBuf::from("config.toml")
        );
        assert_eq!(
            derive_output_name(Path::new("settings.sample.yaml"), Format::Yaml),
            PathBuf::from("settings.yaml")
        );
    }

    #[test]
    fn derives_output_name_marker_after_extension() {
        assert_eq!(
            derive_output_name(Path::new("config.toml.example"), Format::Toml),
            PathBuf::from("config.toml")
        );
    }

    #[test]
    fn derives_output_name_preserves_directory() {
        assert_eq!(
            derive_output_name(Path::new("conf/config.example.toml"), Format::Toml),
            PathBuf::from("conf/config.toml")
        );
    }

    #[test]
    fn derives_output_name_replaces_extension_that_mismatches_detected_format() {
        // Misnamed template: extension says YAML, content sniffed as JSON —
        // the written file's name must reflect the format actually used.
        assert_eq!(
            derive_output_name(Path::new("settings.example.yaml"), Format::Json),
            PathBuf::from("settings.json")
        );
    }

    #[test]
    fn derives_output_name_appends_extension_when_missing() {
        assert_eq!(
            derive_output_name(Path::new("config.example"), Format::Toml),
            PathBuf::from("config.toml")
        );
        assert_eq!(
            derive_output_name(Path::new("settings.sample"), Format::Yaml),
            PathBuf::from("settings.yaml")
        );
    }

    #[test]
    fn detect_format_from_content_recognizes_each_format() {
        assert_eq!(
            detect_format_from_content("host = \"localhost\"\nport = 5432\n"),
            Some(Format::Toml)
        );
        assert_eq!(
            detect_format_from_content("host: localhost\nport: 5432\n"),
            Some(Format::Yaml)
        );
        assert_eq!(
            detect_format_from_content(r#"{"host": "localhost", "port": 5432}"#),
            Some(Format::Json)
        );
        // JSONC comments are still valid JSON content.
        assert_eq!(
            detect_format_from_content("{\n  // comment\n  \"host\": \"localhost\"\n}"),
            Some(Format::Json)
        );
    }

    #[test]
    fn detect_format_from_content_prefers_json_over_yaml_for_object_syntax() {
        // A JSON object is also valid YAML, so JSON must win the race.
        assert_eq!(
            detect_format_from_content(r#"{"a": 1}"#),
            Some(Format::Json)
        );
    }

    #[test]
    fn detect_format_from_content_rejects_dotenv_shape() {
        assert_eq!(detect_format_from_content("KEY=value\nOTHER=1\n"), None);
    }

    #[test]
    fn detect_format_from_content_rejects_inconclusive_content() {
        assert_eq!(detect_format_from_content(""), None);
        assert_eq!(detect_format_from_content("# just a comment\n"), None);
        // A bare JSON array isn't the object shape env-wizard prompts over.
        assert_eq!(detect_format_from_content("[1, 2, 3]"), None);
    }

    #[test]
    fn resolve_template_format_prefers_content_over_extension() {
        // Misnamed: extension says JSON, content is actually TOML.
        assert_eq!(
            resolve_template_format(
                Path::new("config.example.json"),
                "host = \"localhost\"\nport = 5432\n"
            ),
            Some(Format::Toml)
        );
    }

    #[test]
    fn resolve_template_format_falls_back_to_extension_when_content_is_inconclusive() {
        assert_eq!(
            resolve_template_format(Path::new("config.example.yaml"), ""),
            Some(Format::Yaml)
        );
    }

    #[test]
    fn resolve_template_format_detects_extensionless_template_from_content() {
        assert_eq!(
            resolve_template_format(Path::new("config.example"), "host = \"localhost\"\n"),
            Some(Format::Toml)
        );
    }

    #[test]
    fn resolve_template_format_requires_a_template_marker() {
        assert_eq!(
            resolve_template_format(Path::new("config.toml"), "host = \"localhost\"\n"),
            None
        );
    }

    #[test]
    fn classifies_scalars() {
        assert_eq!(classify("5432"), ScalarType::Int);
        assert_eq!(classify("1.5"), ScalarType::Float);
        assert_eq!(classify("true"), ScalarType::Bool);
        assert_eq!(classify("null"), ScalarType::Null);
        assert_eq!(classify("localhost"), ScalarType::String);
        assert_eq!(classify(""), ScalarType::String);
    }

    #[test]
    fn apply_answers_skips_unchanged() {
        struct Recorder {
            calls: Vec<(Vec<String>, String)>,
        }
        impl ConfigDoc for Recorder {
            fn fields(&self) -> Vec<Field> {
                vec![]
            }
            fn set(&mut self, path: &[String], value: &str) -> Result<()> {
                self.calls.push((path.to_vec(), value.to_string()));
                Ok(())
            }
            fn render(&self) -> String {
                String::new()
            }
        }

        let fields = vec![
            Field {
                display: "a".into(),
                path: vec!["a".into()],
                original: "1".into(),
                description: String::new(),
            },
            Field {
                display: "b".into(),
                path: vec!["b".into()],
                original: "2".into(),
                description: String::new(),
            },
        ];
        let answers = vec!["1".to_string(), "changed".to_string()];

        let mut doc = Recorder { calls: vec![] };
        apply_answers(&mut doc, &fields, &answers).unwrap();

        assert_eq!(
            doc.calls,
            vec![(vec!["b".to_string()], "changed".to_string())]
        );
    }
}
