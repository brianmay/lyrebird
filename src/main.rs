mod ffprobe;
mod manifest;
mod rename_plan;
mod tmdb;
mod validate;

use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "lyrebird",
    version,
    about = "Identify and rename HandBrake rips using TMDB metadata"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Resolve a TMDB manifest into a rename plan (renames.txt)
    Resolve {
        /// Tab-separated manifest: source, kind (tv/movie/manual), kind-specific columns
        manifest: PathBuf,
        /// Where to write the resolved rename plan
        #[arg(short, long, default_value = "renames.txt")]
        output: PathBuf,
    },
    /// Check a rename plan for errors without touching the filesystem
    Validate {
        /// Rename plan produced by `resolve`
        plan: PathBuf,
    },
    /// Execute the renames in a plan
    Apply {
        /// Rename plan produced by `resolve`
        plan: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Resolve { manifest, output } => {
            let rows = manifest::parse(&manifest)?;
            let tmdb = tmdb::Tmdb::from_env()?;
            let plans = rename_plan::resolve(&rows, &tmdb)?;
            for plan in &plans {
                println!("{} -> {}", plan.old, plan.new);
            }
            rename_plan::write(&plans, &output)?;
            println!("wrote {} rename(s) to {}", plans.len(), output.display());
            Ok(())
        }
        Command::Validate { plan } => {
            let entries = rename_plan::read(&plan)?;
            let issues = validate::validate(&entries, Path::new("."));
            for issue in &issues {
                println!("{issue}");
            }
            let errors = issues
                .iter()
                .filter(|i| i.severity == validate::Severity::Error)
                .count();
            if errors > 0 {
                anyhow::bail!("{errors} error(s) found — fix the plan before applying");
            }
            println!("plan OK: {} rename(s), all checks passed", entries.len());
            Ok(())
        }
        Command::Apply { plan } => {
            anyhow::bail!("apply {}: not yet implemented", plan.display())
        }
    }
}
