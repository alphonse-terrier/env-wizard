//! Parsing of a `.env.example` file into an ordered list of variables,
//! keeping the comments preceding each variable as its description.

/// A variable declared in the `.env.example`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvVar {
    /// Variable name (e.g. `DATABASE_URL`).
    pub key: String,
    /// Default value found in the example, if present and non-empty.
    pub default: Option<String>,
    /// Help comment accumulated above the variable.
    pub description: String,
}

/// Parse the content of a `.env.example`.
///
/// - Comment lines (`#…`) are accumulated and become the `description` of the
///   next variable encountered.
/// - Assignments `KEY=VALUE` (with an optional `export ` prefix) create an
///   `EnvVar`. The default value is unquoted (`"…"` / `'…'`).
/// - `KEY=` with no value → `default = None`.
/// - Blank lines are ignored (and reset the comment buffer).
pub fn parse(content: &str) -> Vec<EnvVar> {
    let mut vars = Vec::new();
    let mut comment_buf: Vec<String> = Vec::new();

    for raw_line in content.lines() {
        let line = raw_line.trim();

        if line.is_empty() {
            // A blank line separates blocks: drop the current comment.
            comment_buf.clear();
            continue;
        }

        if let Some(comment) = line.strip_prefix('#') {
            comment_buf.push(comment.trim().to_string());
            continue;
        }

        // Attempt a KEY=VALUE assignment.
        let assignment = line.strip_prefix("export ").unwrap_or(line);
        if let Some((key_part, value_part)) = assignment.split_once('=') {
            let key = key_part.trim();
            if key.is_empty() || !is_valid_key(key) {
                // Not a real variable declaration: ignore it.
                comment_buf.clear();
                continue;
            }

            let default = normalize_default(value_part.trim());
            // Keep comment lines separate so they render as a multi-line hint.
            let description = comment_buf.join("\n");

            vars.push(EnvVar {
                key: key.to_string(),
                default,
                description,
            });
            comment_buf.clear();
        } else {
            // Unrecognized line: reset the context.
            comment_buf.clear();
        }
    }

    vars
}

/// A valid variable name: letters, digits, underscore; does not start with a digit.
fn is_valid_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Unquote and clean a default value. Returns `None` if empty.
fn normalize_default(value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }

    let unquoted = if value.len() >= 2 {
        let bytes = value.as_bytes();
        let first = bytes[0];
        let last = bytes[value.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            &value[1..value.len() - 1]
        } else {
            value
        }
    } else {
        value
    };

    if unquoted.is_empty() {
        None
    } else {
        Some(unquoted.to_string())
    }
}
