//! TOML config template support, backed by `toml_edit`'s format-preserving DOM.
//!
//! Only scalar leaves (string/int/float/bool) are exposed as [`Field`]s and
//! editable via [`ConfigDoc::set`]. Datetimes, arrays, and inline tables are
//! left untouched in v1 (not offered to the user).

use anyhow::{bail, Context, Result};
use toml_edit::{DocumentMut, Item, Table, Value};

use super::{ConfigDoc, Field, ScalarType};

pub struct TomlDoc {
    doc: DocumentMut,
}

impl TomlDoc {
    pub fn parse(content: &str) -> Result<Self> {
        let doc: DocumentMut = content.parse().context("failed to parse TOML")?;
        Ok(Self { doc })
    }

    /// True if the document has no top-level keys. Used by content-based
    /// format sniffing: an empty or comment-only file parses successfully as
    /// TOML (an empty table) for every format, so that alone can't decide
    /// anything — a non-empty table is what makes a sniff decisive.
    pub fn is_empty(&self) -> bool {
        self.doc.as_table().is_empty()
    }
}

impl ConfigDoc for TomlDoc {
    fn fields(&self) -> Vec<Field> {
        let mut out = Vec::new();
        collect_fields(self.doc.as_table(), &[], &mut out);
        out
    }

    fn set(&mut self, path: &[String], value: &str) -> Result<()> {
        set_path(self.doc.as_table_mut(), path, value)
    }

    fn render(&self) -> String {
        self.doc.to_string()
    }
}

fn collect_fields(table: &Table, prefix: &[String], out: &mut Vec<Field>) {
    for (key_str, item) in table.iter() {
        let mut path = prefix.to_vec();
        path.push(key_str.to_string());

        match item {
            Item::Table(sub) => collect_fields(sub, &path, out),
            Item::Value(value) => {
                let Some(_) = scalar_type_of(value) else {
                    // Datetime / array / inline table: not offered in v1.
                    continue;
                };
                let key = table
                    .key(key_str)
                    .expect("key must exist for a key just yielded by iter()");
                out.push(Field {
                    display: path.join("."),
                    path,
                    original: render_scalar(value),
                    description: describe(key, value),
                });
            }
            Item::ArrayOfTables(_) | Item::None => {}
        }
    }
}

fn scalar_type_of(value: &Value) -> Option<ScalarType> {
    match value {
        Value::Integer(_) => Some(ScalarType::Int),
        Value::Float(_) => Some(ScalarType::Float),
        Value::Boolean(_) => Some(ScalarType::Bool),
        Value::String(_) => Some(ScalarType::String),
        // Datetime / Array / InlineTable are not scalar leaves in v1.
        _ => None,
    }
}

fn render_scalar(value: &Value) -> String {
    match value {
        Value::String(f) => f.value().clone(),
        Value::Integer(f) => f.value().to_string(),
        Value::Float(f) => f.value().to_string(),
        Value::Boolean(f) => f.value().to_string(),
        _ => String::new(),
    }
}

/// Leading standalone comment lines above the key, plus a trailing same-line
/// comment after the value, if any.
fn describe(key: &toml_edit::Key, value: &Value) -> String {
    let mut lines = Vec::new();

    if let Some(prefix) = key.leaf_decor().prefix().and_then(|r| r.as_str()) {
        for line in prefix.lines() {
            if let Some(comment) = line.trim().strip_prefix('#') {
                lines.push(comment.trim().to_string());
            }
        }
    }

    if let Some(suffix) = value.decor().suffix().and_then(|r| r.as_str()) {
        if let Some(comment) = suffix.trim().strip_prefix('#') {
            let comment = comment.trim();
            if !comment.is_empty() {
                lines.push(comment.to_string());
            }
        }
    }

    lines.join("\n")
}

fn set_path(table: &mut Table, path: &[String], value: &str) -> Result<()> {
    match path {
        [] => bail!("cannot set an empty path"),
        [last] => {
            let item = table
                .get_mut(last)
                .with_context(|| format!("key not found while writing back: {last}"))?;
            set_scalar(item, value)
        }
        [head, rest @ ..] => {
            let item = table
                .get_mut(head)
                .with_context(|| format!("key not found while writing back: {head}"))?;
            let sub = item
                .as_table_mut()
                .with_context(|| format!("{head} is not a table"))?;
            set_path(sub, rest, value)
        }
    }
}

/// Replaces the scalar at `item` with `input` coerced to the original's type,
/// preserving the original's surrounding decor (whitespace + comments) so
/// only the literal value text changes.
fn set_scalar(item: &mut Item, input: &str) -> Result<()> {
    let old = item.as_value().context("target is not a scalar value")?;
    let old_type = scalar_type_of(old).context("target's type isn't editable in v1")?;
    let decor = old.decor().clone();

    let mut new_value = coerce(old_type, input);
    *new_value.decor_mut() = decor;
    *item = Item::Value(new_value);
    Ok(())
}

/// Coerces `input` to a `Value` matching `old_type`, falling back to a plain
/// string if it doesn't parse as that type (e.g. the user typed non-numeric
/// text into a field that was originally an integer).
fn coerce(old_type: ScalarType, input: &str) -> Value {
    match old_type {
        ScalarType::Int => input
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or_else(|_| Value::from(input)),
        ScalarType::Float => input
            .parse::<f64>()
            .map(Value::from)
            .unwrap_or_else(|_| Value::from(input)),
        ScalarType::Bool => match input {
            "true" => Value::from(true),
            "false" => Value::from(false),
            _ => Value::from(input),
        },
        ScalarType::Null | ScalarType::String => Value::from(input),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::apply_answers;

    const EXAMPLE: &str = r#"# Database connection
[database]
# Hostname to connect to
host = "localhost"
port = 5432 # default port
enabled = true

[cache]
ttl = 60
"#;

    #[test]
    fn fields_have_dotted_paths_defaults_and_comments() {
        let doc = TomlDoc::parse(EXAMPLE).unwrap();
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
        let mut doc = TomlDoc::parse(EXAMPLE).unwrap();
        let fields = doc.fields();
        let answers: Vec<String> = fields.iter().map(|f| f.original.clone()).collect();
        apply_answers(&mut doc, &fields, &answers).unwrap();
        assert_eq!(doc.render(), EXAMPLE);
    }

    #[test]
    fn changed_value_keeps_type() {
        let mut doc = TomlDoc::parse(EXAMPLE).unwrap();
        doc.set(&["database".to_string(), "port".to_string()], "6543")
            .unwrap();
        let out = doc.render();
        assert!(out.contains("port = 6543 # default port"));
        assert!(!out.contains("port = \"6543\""));
    }

    #[test]
    fn changed_string_value_is_quoted() {
        let mut doc = TomlDoc::parse(EXAMPLE).unwrap();
        doc.set(&["database".to_string(), "host".to_string()], "db.prod")
            .unwrap();
        let out = doc.render();
        assert!(out.contains("host = \"db.prod\""));
    }
}
