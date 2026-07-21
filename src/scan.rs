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
            // C#
            r#"Environment\.GetEnvironmentVariable\(\s*["']([^"']+)["']"#,
            // Java / Kotlin
            r#"System\.getenv\(\s*["']([^"']+)["']"#,
        ]
        .iter()
        .map(|p| Regex::new(p).expect("valid scan regex"))
        .collect()
    })
}

/// Truncates `line` at the start of a `//` or `#` comment marker that appears
/// outside a quoted string, so a commented-out access idiom (`// process.env.OLD`,
/// `# os.getenv("OLD")`) isn't reported as real usage. Best-effort heuristic —
/// quote state resets at the start of each line and escape sequences inside
/// strings aren't modeled — but it's enough to avoid the common case, and a URL
/// like `"http://example.com"` is correctly left alone since its `//` sits
/// inside a string.
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'#' if !in_single && !in_double => return &line[..i],
            b'/' if !in_single && !in_double && bytes.get(i + 1) == Some(&b'/') => {
                return &line[..i];
            }
            _ => {}
        }
        i += 1;
    }
    line
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
            let line = strip_comment(line);
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
        // Unique per call (not just per file count) so concurrently-running
        // tests never share a temp dir and race on remove_dir_all/create.
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("env-wizard-scan-{id}"));
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
            ("g.cs", "Environment.GetEnvironmentVariable(\"DOTNET_ENV\")"),
            ("h.java", "System.getenv(\"JAVA_HOME_VAR\")"),
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
            "DOTNET_ENV",
            "JAVA_HOME_VAR",
        ] {
            assert!(found.contains_key(key), "missing {key} in {found:?}");
        }
    }

    #[test]
    fn skips_commented_out_access() {
        let found = scan_dir(&[
            (
                "a.js",
                "// const x = process.env.COMMENTED_JS;\nconst y = process.env.REAL_JS;",
            ),
            ("b.py", "# os.getenv('COMMENTED_PY')\nos.getenv('REAL_PY')"),
        ]);
        assert!(found.contains_key("REAL_JS"), "{found:?}");
        assert!(found.contains_key("REAL_PY"), "{found:?}");
        assert!(!found.contains_key("COMMENTED_JS"), "{found:?}");
        assert!(!found.contains_key("COMMENTED_PY"), "{found:?}");
    }

    #[test]
    fn url_double_slash_in_string_is_not_mistaken_for_a_comment() {
        let found = scan_dir(&[(
            "a.js",
            "const base = \"http://example.com\"; const k = process.env.AFTER_URL;",
        )]);
        assert!(found.contains_key("AFTER_URL"), "{found:?}");
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
