//! Discovers environment variables *used in the source code* — a complement to
//! the `.env.example`. Regex-based (v1): fast and language-agnostic, matching the
//! common access idioms. Computed/dynamic keys (e.g. `process.env[expr]`) are
//! inherently undetectable this way and are simply not reported.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;

use crate::parser::EnvVar;
use crate::repo;

/// Access-pattern regexes, one capture group each (the variable name).
fn patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            // JS / TS
            r#"process\.env\.([A-Za-z_][A-Za-z0-9_]*)"#,
            r#"process\.env\[\s*["']([^"']+)["']\s*\]"#,
            r#"import\.meta\.env\.([A-Za-z_][A-Za-z0-9_]*)"#,
            // Python
            r#"os\.environ\[\s*["']([^"']+)["']\s*\]"#,
            r#"os\.environ\.get\(\s*["']([^"']+)["']"#,
            r#"os\.getenv\(\s*["']([^"']+)["']"#,
            // Rust
            r#"env::var(?:_os)?\(\s*["']([^"']+)["']"#,
            r#"env!\(\s*["']([^"']+)["']"#,
            r#"option_env!\(\s*["']([^"']+)["']"#,
            // Go
            r#"os\.(?:Getenv|LookupEnv)\(\s*["']([^"']+)["']"#,
            // Ruby
            r#"ENV\[\s*["']([^"']+)["']\s*\]"#,
            r#"ENV\.fetch\(\s*["']([^"']+)["']"#,
            // PHP
            r#"getenv\(\s*["']([^"']+)["']"#,
            r#"\$_ENV\[\s*["']([^"']+)["']\s*\]"#,
        ]
        .iter()
        .map(|p| Regex::new(p).expect("valid scan regex"))
        .collect()
    })
}

/// A syntactically valid environment variable name.
fn is_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Scans `repo_root` for environment variables used in code.
///
/// Returns each discovered name mapped to the first `"file:line"` where it was
/// seen. Never reads `.env`/lockfiles/oversized/binary files (see [`repo`]).
pub fn scan_env_vars(repo_root: &Path) -> BTreeMap<String, String> {
    let mut found: BTreeMap<String, String> = BTreeMap::new();
    let regexes = patterns();

    for (path, content) in repo::text_files(repo_root) {
        let rel = path
            .strip_prefix(repo_root)
            .unwrap_or(&path)
            .display()
            .to_string();
        for (i, line) in content.lines().enumerate() {
            for re in regexes {
                for caps in re.captures_iter(line) {
                    if let Some(m) = caps.get(1) {
                        let name = m.as_str();
                        if is_valid_name(name) {
                            found
                                .entry(name.to_string())
                                .or_insert_with(|| format!("{rel}:{}", i + 1));
                        }
                    }
                }
            }
        }
    }

    found
}

/// Turns scan results into `EnvVar`s for the wizard, recording provenance in the
/// description so the prompt and generated `.env` note where each was detected.
pub fn to_env_vars(found: &BTreeMap<String, String>) -> Vec<EnvVar> {
    found
        .iter()
        .map(|(name, loc)| EnvVar {
            key: name.clone(),
            default: None,
            description: format!("detected in code: {loc}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_dir(files: &[(&str, &str)]) -> BTreeMap<String, String> {
        let dir = std::env::temp_dir().join(format!("env-wizard-scan-{}", files.len()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, body) in files {
            std::fs::write(dir.join(name), body).unwrap();
        }
        let out = scan_env_vars(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    #[test]
    fn detects_each_language() {
        let found = scan_dir(&[
            (
                "a.js",
                "const x = process.env.API_URL;\nconst y = import.meta.env.VITE_KEY;",
            ),
            ("b.py", "os.getenv('DB_HOST')\nos.environ[\"DB_PORT\"]"),
            (
                "c.rs",
                "std::env::var(\"RUST_LOG\").ok();\nenv!(\"BUILD_ID\");",
            ),
            ("d.go", "os.Getenv(\"GO_ENV\")"),
            ("e.rb", "ENV['RAILS_ENV']"),
            ("f.php", "getenv('PHP_KEY');"),
        ]);
        for key in [
            "API_URL",
            "VITE_KEY",
            "DB_HOST",
            "DB_PORT",
            "RUST_LOG",
            "BUILD_ID",
            "GO_ENV",
            "RAILS_ENV",
            "PHP_KEY",
        ] {
            assert!(found.contains_key(key), "missing {key} in {found:?}");
        }
    }

    #[test]
    fn records_first_location() {
        let found = scan_dir(&[("app.js", "line0\nconst a = process.env.TOKEN;")]);
        assert_eq!(found.get("TOKEN").map(String::as_str), Some("app.js:2"));
    }

    #[test]
    fn never_scans_dotenv_secrets() {
        // A real .env referencing a var must not surface it.
        let found = scan_dir(&[
            (".env", "process.env.LEAKED_FROM_ENV"),
            ("ok.js", "process.env.REAL_ONE"),
        ]);
        assert!(found.contains_key("REAL_ONE"), "{found:?}");
        assert!(!found.contains_key("LEAKED_FROM_ENV"), "{found:?}");
    }
}
