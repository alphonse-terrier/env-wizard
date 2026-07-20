//! Writes the resulting `.env` file, preserving order and confirming before
//! overwriting an existing file.

use std::path::Path;

use anyhow::{Context, Result};
use console::style;
use dialoguer::{theme::ColorfulTheme, Confirm};

use crate::parser::EnvVar;

/// Outcome of the write step.
pub enum WriteOutcome {
    /// The file was written to this path.
    Written,
    /// The user declined to overwrite an existing file.
    Aborted,
}

/// Writes `entries` (variable + chosen value) to `output_path`, preserving each
/// variable's `.env.example` comment as a `#` header above its `KEY=value`.
///
/// If the file already exists and `assume_yes` is false, asks for confirmation.
/// When overwriting, the previous file is backed up to `<output>.bak`. On Unix,
/// the written file (and backup) are restricted to `0600` since it holds secrets.
pub fn write_env(
    output_path: &Path,
    entries: &[(EnvVar, String)],
    assume_yes: bool,
) -> Result<WriteOutcome> {
    if output_path.exists() && !assume_yes {
        let confirmed = Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt(format!(
                "{} already exists. Overwrite it?",
                output_path.display()
            ))
            .default(false)
            .interact()?;
        if !confirmed {
            return Ok(WriteOutcome::Aborted);
        }
    }

    if output_path.exists() {
        let backup = backup_path(output_path);
        std::fs::copy(output_path, &backup)
            .with_context(|| format!("failed to back up existing file to {}", backup.display()))?;
        restrict_permissions(&backup);
        println!("{} {}", style("↳ backup:").dim(), backup.display());
    }

    let content = render(entries);
    std::fs::write(output_path, content)
        .with_context(|| format!("failed to write {}", output_path.display()))?;
    restrict_permissions(output_path);

    Ok(WriteOutcome::Written)
}

/// On Unix, restrict a secrets file to owner read/write (`0600`). No-op elsewhere.
#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

/// Builds the `<output>.bak` sibling path.
fn backup_path(output_path: &Path) -> std::path::PathBuf {
    let mut name = output_path.file_name().unwrap_or_default().to_os_string();
    name.push(".bak");
    output_path.with_file_name(name)
}

/// Renders the entries into `.env` text: each variable's comment (if any) as
/// `#` lines, then `KEY=value`, with a blank line between entries.
fn render(entries: &[(EnvVar, String)]) -> String {
    let mut blocks: Vec<String> = Vec::with_capacity(entries.len());
    for (var, value) in entries {
        let mut block = String::new();
        for line in var.description.lines() {
            block.push_str("# ");
            block.push_str(line);
            block.push('\n');
        }
        block.push_str(&var.key);
        block.push('=');
        block.push_str(&maybe_quote(value));
        blocks.push(block);
    }
    let mut out = blocks.join("\n\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// Wraps a value in double quotes if it contains whitespace or special chars.
fn maybe_quote(value: &str) -> String {
    let needs_quote = value
        .chars()
        .any(|c| c.is_whitespace() || c == '#' || c == '"' || c == '\'');
    if needs_quote {
        let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(key: &str, desc: &str) -> EnvVar {
        EnvVar {
            key: key.into(),
            default: None,
            description: desc.into(),
        }
    }

    #[test]
    fn quotes_only_when_needed() {
        assert_eq!(maybe_quote("simple"), "simple");
        assert_eq!(
            maybe_quote("postgres://localhost/db"),
            "postgres://localhost/db"
        );
        assert_eq!(maybe_quote("has space"), "\"has space\"");
        assert_eq!(maybe_quote("has#hash"), "\"has#hash\"");
        assert_eq!(maybe_quote("a\"b"), "\"a\\\"b\"");
    }

    #[test]
    fn render_emits_comments_and_blank_lines() {
        let entries = vec![
            (
                var("DATABASE_URL", "Postgres DSN\nFormat: postgres://…"),
                "postgres://x".to_string(),
            ),
            (var("SECRET_KEY", ""), "s3cr3t val".to_string()),
        ];
        let out = render(&entries);
        assert_eq!(
            out,
            "# Postgres DSN\n# Format: postgres://…\nDATABASE_URL=postgres://x\n\nSECRET_KEY=\"s3cr3t val\"\n"
        );
    }

    #[test]
    fn backup_path_appends_bak() {
        assert_eq!(
            backup_path(Path::new("/tmp/.env")),
            Path::new("/tmp/.env.bak")
        );
    }

    #[test]
    fn write_env_creates_file_and_restricts_perms() {
        let dir = std::env::temp_dir().join("env-wizard-writer-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join(".env");
        let entries = vec![(var("PORT", "the port"), "8080".to_string())];

        write_env(&out, &entries, true).unwrap();

        let content = std::fs::read_to_string(&out).unwrap();
        assert_eq!(content, "# the port\nPORT=8080\n");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&out).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "written .env should be 0600");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
