//! env-wizard — interactive assistant to fill a `.env` from a `.env.example`,
//! with on-demand AI hints from a configurable AI provider (cloud or local).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use console::style;

use env_wizard::parser::{self, EnvVar};
use env_wizard::prompt::{self, Options, Outcome};
use env_wizard::provider;
use env_wizard::scan;
use env_wizard::writer::{self, WriteOutcome};

/// Common names for the example file, tried in order when `--input` is omitted.
const EXAMPLE_ALIASES: &[&str] = &[
    ".env.example",
    ".env.sample",
    ".env.dist",
    ".env.template",
    "env.example",
];

/// Interactive `.env` filler driven by your project's `.env.example`.
#[derive(Parser, Debug)]
#[command(name = "env-wizard", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Path to the example file to read. If omitted, tries common names in
    /// order: .env.example, .env.sample, .env.dist, .env.template, env.example.
    #[arg(short, long)]
    input: Option<PathBuf>,

    /// Path to the env file to write.
    #[arg(short, long, default_value = ".env")]
    output: PathBuf,

    /// Accept all default values and overwrite without confirmation.
    #[arg(short = 'y', long)]
    yes: bool,

    /// Disable the AI hint feature (no calls to the AI provider).
    #[arg(long)]
    no_ai: bool,

    /// Also include environment variables discovered in the code, not just the
    /// ones declared in the example file.
    #[arg(long)]
    from_code: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Choose or change the AI provider used for hints.
    Config,
    /// Audit: compare variables used in code against the `.env.example`.
    Scan,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let input = resolve_input_path(cli.input.as_deref());

    match cli.command {
        Some(Commands::Config) => {
            provider::configure_interactive()?;
            return Ok(());
        }
        Some(Commands::Scan) => return run_scan(&input),
        None => {}
    }

    let repo_root = repo_root_of(&input);
    let vars = collect_vars(&cli, &input, &repo_root)?;

    if vars.is_empty() {
        println!(
            "{}",
            style(format!("No variables found in {}.", input.display())).yellow()
        );
        return Ok(());
    }

    let opts = Options {
        no_ai: cli.no_ai,
        accept_defaults: cli.yes,
    };

    let answers = match prompt::run(&repo_root, &vars, &opts)? {
        Outcome::Completed(a) => a,
        Outcome::Quit => {
            println!("{}", style("Aborted — no file written.").yellow());
            return Ok(());
        }
    };

    match writer::write_env(&cli.output, &answers, cli.yes)? {
        WriteOutcome::Written => {
            println!(
                "{} {}",
                style("✓ Wrote").green().bold(),
                cli.output.display()
            );
        }
        WriteOutcome::Aborted => {
            println!(
                "{}",
                style("Overwrite cancelled — no file written.").yellow()
            );
        }
    }

    Ok(())
}

/// Resolves which example file to read, relative to the current directory.
///
/// An explicit `--input` is honored verbatim — no substitution. Otherwise, the
/// first [`EXAMPLE_ALIASES`] entry that exists on disk is used; if none exist,
/// returns the conventional `.env.example` so downstream "no example" messaging
/// (and the code-scan fallback) stays unchanged.
fn resolve_input_path(explicit: Option<&Path>) -> PathBuf {
    resolve_input_path_from(Path::new("."), explicit)
}

/// Same as [`resolve_input_path`], but checks alias existence under `base`
/// instead of the current directory — lets tests probe a temp dir without
/// mutating the process-global current directory.
fn resolve_input_path_from(base: &Path, explicit: Option<&Path>) -> PathBuf {
    if let Some(path) = explicit {
        return path.to_path_buf();
    }
    for name in EXAMPLE_ALIASES {
        if base.join(name).exists() {
            if *name != EXAMPLE_ALIASES[0] {
                println!(
                    "{}",
                    style(format!("Using {name} (no {} found).", EXAMPLE_ALIASES[0])).dim()
                );
            }
            return PathBuf::from(name);
        }
    }
    PathBuf::from(EXAMPLE_ALIASES[0])
}

/// The directory to scan / gather context from: the input file's parent, or cwd.
fn repo_root_of(input: &Path) -> PathBuf {
    input
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Builds the list of variables to prompt for, applying the example file, the
/// `--from-code` augmentation, and the no-example fallback.
fn collect_vars(cli: &Cli, input: &Path, repo_root: &Path) -> Result<Vec<EnvVar>> {
    let example = match std::fs::read_to_string(input) {
        Ok(content) => Some(parser::parse(&content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("could not read {}", input.display())),
    };

    match (example, cli.from_code) {
        (Some(ex), false) => Ok(ex),
        (Some(ex), true) => {
            let code = scan::to_env_vars(&scan::scan_env_vars(repo_root));
            Ok(merge_vars(ex, code))
        }
        (None, _) => {
            // Fallback: no example file — derive the list from the code.
            let found = scan::scan_env_vars(repo_root);
            if found.is_empty() {
                anyhow::bail!(
                    "could not find {} and detected no environment variables in the code — \
                     run env-wizard from the repo root, or pass --input",
                    input.display()
                );
            }
            println!(
                "{}",
                style(format!(
                    "No {} found — using {} variable(s) detected in the code.",
                    input.display(),
                    found.len()
                ))
                .yellow()
            );
            Ok(scan::to_env_vars(&found))
        }
    }
}

/// Appends code-discovered vars whose key isn't already declared in `example`.
fn merge_vars(mut example: Vec<EnvVar>, code: Vec<EnvVar>) -> Vec<EnvVar> {
    let declared: HashSet<String> = example.iter().map(|v| v.key.clone()).collect();
    for v in code {
        if !declared.contains(&v.key) {
            example.push(v);
        }
    }
    example
}

/// `env-wizard scan`: read-only audit of code usage vs the example file.
fn run_scan(input: &Path) -> Result<()> {
    let repo_root = repo_root_of(input);
    let used = scan::scan_env_vars(&repo_root);

    let declared: Vec<String> = match std::fs::read_to_string(input) {
        Ok(content) => parser::parse(&content).into_iter().map(|v| v.key).collect(),
        Err(_) => Vec::new(),
    };
    let declared_set: HashSet<&str> = declared.iter().map(String::as_str).collect();
    let used_set: HashSet<&str> = used.keys().map(String::as_str).collect();

    // No example file: just list what was discovered in the code.
    if declared.is_empty() {
        println!(
            "{}",
            style(format!(
                "No {} — variables detected in code:",
                input.display()
            ))
            .bold()
        );
        if used.is_empty() {
            println!("  (none)");
        }
        for (name, loc) in &used {
            println!(
                "  {} {}",
                style(format!("• {name}")).cyan(),
                style(loc).dim()
            );
        }
        return Ok(());
    }

    let missing: Vec<(&String, &String)> = used
        .iter()
        .filter(|(k, _)| !declared_set.contains(k.as_str()))
        .collect();
    let unused: Vec<&String> = declared
        .iter()
        .filter(|k| !used_set.contains(k.as_str()))
        .collect();

    if !missing.is_empty() {
        println!(
            "{}",
            style(format!(
                "Used in code but missing from {} ({}):",
                input.display(),
                missing.len()
            ))
            .yellow()
            .bold()
        );
        for (name, loc) in &missing {
            println!(
                "  {} {}",
                style(format!("• {name}")).yellow(),
                style(loc).dim()
            );
        }
        println!();
    }

    if !unused.is_empty() {
        println!(
            "{}",
            style(format!(
                "Declared in {} but not found in code ({}):",
                input.display(),
                unused.len()
            ))
            .dim()
            .bold()
        );
        for name in &unused {
            println!("  {}", style(format!("• {name}")).dim());
        }
        println!();
    }

    if missing.is_empty() && unused.is_empty() {
        println!(
            "{}",
            style(format!("✓ In sync — {} variable(s).", declared.len()))
                .green()
                .bold()
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, uniquely-named temp dir for a test; removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("env-wizard-main-test-{name}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn explicit_input_always_wins() {
        let dir = TempDir::new("explicit-wins");
        std::fs::write(dir.0.join(".env.example"), "").unwrap();
        std::fs::write(dir.0.join("foo.txt"), "").unwrap();

        let resolved = resolve_input_path_from(&dir.0, Some(Path::new("foo.txt")));
        assert_eq!(resolved, PathBuf::from("foo.txt"));
    }

    #[test]
    fn alias_priority_prefers_conventional_default() {
        let dir = TempDir::new("alias-priority");
        std::fs::write(dir.0.join(".env.sample"), "").unwrap();
        assert_eq!(
            resolve_input_path_from(&dir.0, None),
            PathBuf::from(".env.sample")
        );

        std::fs::write(dir.0.join(".env.example"), "").unwrap();
        assert_eq!(
            resolve_input_path_from(&dir.0, None),
            PathBuf::from(".env.example")
        );
    }

    #[test]
    fn no_candidate_falls_back_to_conventional_default() {
        let dir = TempDir::new("no-candidate");
        assert_eq!(
            resolve_input_path_from(&dir.0, None),
            PathBuf::from(".env.example")
        );
    }
}
