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

/// Interactive `.env` filler driven by your project's `.env.example`.
#[derive(Parser, Debug)]
#[command(name = "env-wizard", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Path to the example file to read.
    #[arg(short, long, default_value = ".env.example")]
    input: PathBuf,

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

    match cli.command {
        Some(Commands::Config) => {
            provider::configure_interactive()?;
            return Ok(());
        }
        Some(Commands::Scan) => return run_scan(&cli.input),
        None => {}
    }

    let repo_root = repo_root_of(&cli.input);
    let vars = collect_vars(&cli, &repo_root)?;

    if vars.is_empty() {
        println!(
            "{}",
            style(format!("No variables found in {}.", cli.input.display())).yellow()
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
fn collect_vars(cli: &Cli, repo_root: &Path) -> Result<Vec<EnvVar>> {
    let example = match std::fs::read_to_string(&cli.input) {
        Ok(content) => Some(parser::parse(&content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("could not read {}", cli.input.display())),
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
                    cli.input.display()
                );
            }
            println!(
                "{}",
                style(format!(
                    "No {} found — using {} variable(s) detected in the code.",
                    cli.input.display(),
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
