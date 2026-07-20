//! Gathers context from the repo and sends it to the configured AI provider to
//! get a hint about the value of an environment variable.

use std::io::Write;
use std::path::Path;

use anyhow::Result;
use walkdir::WalkDir;

use crate::provider;

/// Max size (bytes) of the README injected into the prompt.
const README_LIMIT: usize = 8 * 1024;
/// Max size (bytes) per config file injected.
const CONFIG_FILE_LIMIT: usize = 4 * 1024;
/// Max total size of the prompt sent to the provider.
const PROMPT_LIMIT: usize = 12 * 1024;
/// Max number of variable occurrences to include.
const MAX_GREP_HITS: usize = 20;

/// Entry point: produces a hint for `var_key`, or an error.
///
/// Loads the configured provider (running the first-run picker if none is set),
/// then sends the assembled prompt to it.
pub fn get_hint(repo_root: &Path, var_key: &str, description: &str) -> Result<String> {
    let cfg = provider::ensure_configured()?;
    let context = gather_context(repo_root, var_key);
    let prompt = build_prompt(var_key, description, &context);
    run_provider(&cfg, &prompt)
}

/// Collects the relevant repo context for the given variable.
fn gather_context(repo_root: &Path, var_key: &str) -> String {
    let mut sections: Vec<String> = Vec::new();

    if let Some(readme) = read_readme(repo_root) {
        sections.push(format!("## README\n\n{readme}"));
    }

    let configs = read_config_files(repo_root);
    if !configs.is_empty() {
        sections.push(format!("## Configuration files\n\n{configs}"));
    }

    let hits = grep_variable(repo_root, var_key);
    if !hits.is_empty() {
        sections.push(format!("## Occurrences of `{var_key}` in the repo\n\n{hits}"));
    }

    sections.join("\n\n")
}

/// Reads the first README found (case-insensitive), truncated.
fn read_readme(repo_root: &Path) -> Option<String> {
    let entries = std::fs::read_dir(repo_root).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.to_ascii_lowercase().starts_with("readme") {
            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                return Some(truncate(&content, README_LIMIT));
            }
        }
    }
    None
}

/// Reads a few useful config files if they exist at the repo root.
fn read_config_files(repo_root: &Path) -> String {
    let mut out = String::new();
    let entries = match std::fs::read_dir(repo_root) {
        Ok(e) => e,
        Err(_) => return out,
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy().to_string();
        if is_interesting_config(&name) {
            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                out.push_str(&format!(
                    "### {name}\n\n{}\n\n",
                    truncate(&content, CONFIG_FILE_LIMIT)
                ));
            }
        }
    }
    out.trim_end().to_string()
}

/// Recognizes config files likely to document the variables.
fn is_interesting_config(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("docker-compose")
        || lower.starts_with("compose")
        || lower == "makefile"
        || lower == ".env.example"
        || lower.starts_with("settings")
        || lower.starts_with("config")
}

/// Searches for occurrences of `var_key` across the repo's text files.
fn grep_variable(repo_root: &Path, var_key: &str) -> String {
    let mut lines: Vec<String> = Vec::new();

    let walker = WalkDir::new(repo_root)
        .into_iter()
        .filter_entry(|e| !is_excluded(e.path()));

    for entry in walker.flatten() {
        if lines.len() >= MAX_GREP_HITS {
            break;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let content = match std::fs::read_to_string(entry.path()) {
            Ok(c) => c,
            Err(_) => continue, // binary or unreadable: skip
        };
        let rel = entry
            .path()
            .strip_prefix(repo_root)
            .unwrap_or(entry.path())
            .display();

        for (i, line) in content.lines().enumerate() {
            if line.contains(var_key) {
                lines.push(format!("{rel}:{}: {}", i + 1, line.trim()));
                if lines.len() >= MAX_GREP_HITS {
                    break;
                }
            }
        }
    }

    lines.join("\n")
}

/// Directories/files to exclude from the walk.
fn is_excluded(path: &Path) -> bool {
    path.file_name()
        .map(|n| {
            let n = n.to_string_lossy();
            n == ".git" || n == "target" || n == "node_modules" || n == ".venv" || n == "vendor"
        })
        .unwrap_or(false)
}

/// Builds the textual prompt sent to `claude`.
fn build_prompt(var_key: &str, description: &str, context: &str) -> String {
    let desc = if description.trim().is_empty() {
        "(no description in the .env.example)".to_string()
    } else {
        description.to_string()
    };

    let prompt = format!(
        "You are helping a developer fill in a `.env` file in a freshly cloned project.\n\
         They don't know what value to give the environment variable `{var_key}`.\n\n\
         Description found in the .env.example: {desc}\n\n\
         Based ONLY on the repository context below, answer concisely (max 5 lines):\n\
         1. What this variable is for.\n\
         2. What value to set (or the expected format/example).\n\
         3. How to obtain it if not obvious (service, command, file).\n\
         If the context is insufficient to conclude, say so clearly rather than inventing.\n\n\
         --- REPOSITORY CONTEXT ---\n{context}\n"
    );

    truncate(&prompt, PROMPT_LIMIT)
}

/// Sends the prompt to the configured provider, showing a transient spinner.
fn run_provider(cfg: &provider::Config, prompt: &str) -> Result<String> {
    let label = if cfg.label.is_empty() {
        cfg.kind.clone()
    } else {
        cfg.label.clone()
    };
    print!("  ⋯ Fetching a hint via {label}…\r");
    let _ = std::io::stdout().flush();

    let result = provider::run(cfg, prompt);

    // Always clear the transient "Fetching…" line, success or failure.
    print!("\r{}\r", " ".repeat(40 + label.len()));
    let _ = std::io::stdout().flush();

    result
}

/// Truncates a string to `limit` bytes, on a char boundary.
fn truncate(s: &str, limit: usize) -> String {
    if s.len() <= limit {
        return s.to_string();
    }
    let mut end = limit;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[… truncated …]", &s[..end])
}
