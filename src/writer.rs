//! Writes the resulting `.env` file, preserving order and confirming before
//! overwriting an existing file.

use std::path::Path;

use anyhow::{Context, Result};
use console::style;
use dialoguer::{theme::ColorfulTheme, Confirm};

/// Outcome of the write step.
pub enum WriteOutcome {
    /// The file was written to this path.
    Written,
    /// The user declined to overwrite an existing file.
    Aborted,
}

/// Writes `entries` to `output_path` as `KEY=value` lines.
///
/// If the file already exists and `assume_yes` is false, asks for confirmation.
/// When overwriting, the previous file is backed up to `<output>.bak`.
pub fn write_env(
    output_path: &Path,
    entries: &[(String, String)],
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
        println!("{} {}", style("↳ backup:").dim(), backup.display());
    }

    let content = render(entries);
    std::fs::write(output_path, content)
        .with_context(|| format!("failed to write {}", output_path.display()))?;

    Ok(WriteOutcome::Written)
}

/// Builds the `<output>.bak` sibling path.
fn backup_path(output_path: &Path) -> std::path::PathBuf {
    let mut name = output_path.file_name().unwrap_or_default().to_os_string();
    name.push(".bak");
    output_path.with_file_name(name)
}

/// Renders the entries into `.env` text, quoting values when needed.
fn render(entries: &[(String, String)]) -> String {
    let mut out = String::new();
    for (key, value) in entries {
        out.push_str(key);
        out.push('=');
        out.push_str(&maybe_quote(value));
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
