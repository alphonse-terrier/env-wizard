//! env-wizard — interactive assistant to fill a `.env` from a `.env.example`,
//! with on-demand AI hints from a configurable AI provider (cloud or local).

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use console::style;

use env_wizard::parser;
use env_wizard::prompt::{self, Options, Outcome};
use env_wizard::provider;
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
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Choose or change the AI provider used for hints.
    Config,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Provider configuration subcommand: run the picker and exit.
    if let Some(Commands::Config) = cli.command {
        provider::configure_interactive()?;
        return Ok(());
    }

    let content = std::fs::read_to_string(&cli.input).with_context(|| {
        format!(
            "could not read {} — run env-wizard from the repo root, or pass --input",
            cli.input.display()
        )
    })?;

    let vars = parser::parse(&content);
    if vars.is_empty() {
        println!(
            "{}",
            style(format!("No variables found in {}.", cli.input.display())).yellow()
        );
        return Ok(());
    }

    // Repo root for context gathering: the input file's parent, or cwd.
    let repo_root = cli
        .input
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

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
