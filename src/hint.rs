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
/// Skip files larger than this when grepping (avoids loading huge files).
const MAX_GREP_FILE_BYTES: u64 = 256 * 1024;

/// Entry point: produces a hint for `var_key`, or an error.
///
/// Loads the configured provider (running the first-run picker if none is set),
/// then sends the assembled prompt to it. When `question` is `Some`, the model
/// is asked that specific question instead of the generic hint.
pub fn get_hint(
    repo_root: &Path,
    var_key: &str,
    description: &str,
    question: Option<&str>,
) -> Result<String> {
    let cfg = provider::ensure_configured()?;
    let context = gather_context(repo_root, var_key);
    let prompt = build_prompt(var_key, description, &context, question);
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
        sections.push(format!(
            "## Occurrences of `{var_key}` in the repo\n\n{hits}"
        ));
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
        let path = entry.path();
        // Skip lockfiles (huge, noisy) and files over the size cap.
        if is_lockfile(path) {
            continue;
        }
        if entry.metadata().map(|m| m.len()).unwrap_or(0) > MAX_GREP_FILE_BYTES {
            continue;
        }
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue, // binary or unreadable: skip
        };
        let rel = path.strip_prefix(repo_root).unwrap_or(path).display();

        for (i, line) in content.lines().enumerate() {
            if matches_whole_word(line, var_key) {
                lines.push(format!("{rel}:{}: {}", i + 1, line.trim()));
                if lines.len() >= MAX_GREP_HITS {
                    break;
                }
            }
        }
    }

    lines.join("\n")
}

/// True if `needle` appears in `haystack` bounded by non-identifier chars, so
/// `PORT` matches `PORT=1` but not `SUPPORT` or `EXPORT`.
fn matches_whole_word(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let bytes = haystack.as_bytes();
    let mut from = 0;
    while let Some(rel) = haystack[from..].find(needle) {
        let start = from + rel;
        let end = start + needle.len();
        let before_ok = start == 0 || !is_ident(bytes[start - 1] as char);
        let after_ok = end >= bytes.len() || !is_ident(bytes[end] as char);
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

/// Recognizes dependency lockfiles, which are large and add no useful context.
fn is_lockfile(path: &Path) -> bool {
    match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => {
            let lower = name.to_ascii_lowercase();
            lower.ends_with(".lock")
                || lower == "package-lock.json"
                || lower == "yarn.lock"
                || lower == "pnpm-lock.yaml"
                || lower == "poetry.lock"
                || lower == "composer.lock"
        }
        None => false,
    }
}

/// Directories/files to exclude from the walk.
///
/// Beyond the usual build/VCS dirs, this deliberately skips real dotenv files
/// (`.env`, `.env.local`, `.env.production`, …) so their secret *values* can
/// never end up in a prompt sent to the AI. Template files (`.env.example`,
/// `.env.sample`, …) are safe and left in.
fn is_excluded(path: &Path) -> bool {
    path.file_name()
        .map(|n| {
            let n = n.to_string_lossy();
            n == ".git"
                || n == "target"
                || n == "node_modules"
                || n == ".venv"
                || n == "vendor"
                || is_secret_env_file(&n)
        })
        .unwrap_or(false)
}

/// True for dotenv files that may contain real secrets — everything named
/// `.env` or `.env.*` except the safe template variants.
fn is_secret_env_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower != ".env" && !lower.starts_with(".env.") {
        return false;
    }
    !(lower.ends_with(".example")
        || lower.ends_with(".sample")
        || lower.ends_with(".template")
        || lower.ends_with(".dist"))
}

/// Builds the textual prompt sent to the provider.
///
/// With no `question`, asks for a generic 3-point hint. With a `question`, asks
/// the model to answer it directly, still grounded in the repo context.
fn build_prompt(var_key: &str, description: &str, context: &str, question: Option<&str>) -> String {
    let desc = if description.trim().is_empty() {
        "(no description in the .env.example)".to_string()
    } else {
        description.to_string()
    };

    let task = match question.map(str::trim).filter(|q| !q.is_empty()) {
        Some(q) => format!(
            "The developer's specific question about `{var_key}`: \"{q}\"\n\n\
             Answer that question directly and concisely (max 6 lines), using ONLY the \
             repository context below. If the context is insufficient, say so plainly \
             rather than inventing."
        ),
        None => "\
            Based ONLY on the repository context below, answer concisely (max 5 lines):\n\
            1. What this variable is for.\n\
            2. What value to set (or the expected format/example).\n\
            3. How to obtain it if not obvious (service, command, file).\n\
            If the context is insufficient to conclude, say so clearly rather than inventing."
            .to_string(),
    };

    let prompt = format!(
        "You are helping a developer fill in a `.env` file in a freshly cloned project.\n\
         They don't know what value to give the environment variable `{var_key}`.\n\n\
         Description found in the .env.example: {desc}\n\n\
         {task}\n\n\
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_secret_vs_template_env_files() {
        assert!(is_secret_env_file(".env"));
        assert!(is_secret_env_file(".env.local"));
        assert!(is_secret_env_file(".env.production"));
        assert!(is_secret_env_file(".ENV")); // case-insensitive
        assert!(!is_secret_env_file(".env.example"));
        assert!(!is_secret_env_file(".env.sample"));
        assert!(!is_secret_env_file(".env.template"));
        assert!(!is_secret_env_file("settings.py"));
    }

    #[test]
    fn grep_never_reads_dotenv_secrets() {
        let dir = std::env::temp_dir().join("env-wizard-grep-secret-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A real .env with a secret, plus a source file that references the var.
        std::fs::write(dir.join(".env"), "API_TOKEN=supersecret-should-not-leak\n").unwrap();
        std::fs::write(dir.join(".env.local"), "API_TOKEN=also-secret\n").unwrap();
        std::fs::write(dir.join("app.py"), "token = os.environ['API_TOKEN']\n").unwrap();

        let hits = grep_variable(&dir, "API_TOKEN");

        assert!(
            hits.contains("app.py"),
            "should surface the source reference"
        );
        assert!(
            !hits.contains("supersecret-should-not-leak") && !hits.contains("also-secret"),
            "must never include values read from .env files: {hits}"
        );
        // No hit line should originate from a dotenv file (paths are `<file>:<n>:`).
        assert!(
            !hits.contains(".env:") && !hits.contains(".env.local:"),
            "must not read any .env file: {hits}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn whole_word_matching_ignores_substrings() {
        assert!(matches_whole_word("PORT=8080", "PORT"));
        assert!(matches_whole_word("os.environ['PORT']", "PORT"));
        assert!(!matches_whole_word("export SUPPORT_LEVEL=1", "PORT"));
        assert!(!matches_whole_word("IMPORTANT", "PORT"));
    }

    #[test]
    fn grep_skips_lockfiles_and_large_files() {
        let dir = std::env::temp_dir().join("env-wizard-grep-limits-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Cargo.lock"), "MY_VAR = referenced here\n").unwrap();
        let big = format!("MY_VAR\n{}", "x".repeat((MAX_GREP_FILE_BYTES + 1) as usize));
        std::fs::write(dir.join("big.txt"), big).unwrap();
        std::fs::write(dir.join("src.rs"), "let v = env(\"MY_VAR\");\n").unwrap();

        let hits = grep_variable(&dir, "MY_VAR");

        assert!(hits.contains("src.rs"), "should read normal source: {hits}");
        assert!(
            !hits.contains("Cargo.lock"),
            "should skip lockfiles: {hits}"
        );
        assert!(
            !hits.contains("big.txt"),
            "should skip oversized files: {hits}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_prompt_includes_custom_question() {
        let p = build_prompt("API_KEY", "the key", "ctx", Some("what format is it?"));
        assert!(p.contains("what format is it?"), "{p}");
        assert!(p.contains("specific question"), "{p}");
    }

    #[test]
    fn build_prompt_generic_without_question() {
        let p = build_prompt("API_KEY", "the key", "ctx", None);
        assert!(p.contains("What this variable is for"), "{p}");
    }
}
