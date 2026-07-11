mod ffprobe;
mod manifest;
mod rename_plan;
mod sheet;
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
    /// Generate a contact-sheet PNG per video file (identification aid)
    Sheet {
        /// Video files to generate sheets for
        #[arg(required = true)]
        files: Vec<PathBuf>,
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
            let entries = load_validated(&plan)?;
            println!("plan OK: {} rename(s), all checks passed", entries.len());
            Ok(())
        }
        Command::Apply { plan } => {
            // Apply always re-validates: a plan edited or a directory changed
            // since the last `validate` run must not slip through.
            let entries = load_validated(&plan)?;
            rename_plan::apply(&entries, Path::new("."))?;
            println!("applied {} rename(s)", entries.len());
            Ok(())
        }
        Command::Sheet { files } => {
            let mut failures = 0;
            for file in &files {
                match sheet::generate(file) {
                    Ok(output) => println!("{} -> {}", file.display(), output.display()),
                    Err(err) => {
                        eprintln!("ERROR {}: {err:#}", file.display());
                        failures += 1;
                    }
                }
            }
            if failures > 0 {
                anyhow::bail!("{failures} of {} sheet(s) failed", files.len());
            }
            Ok(())
        }
    }
}

fn load_validated(plan: &Path) -> Result<Vec<rename_plan::PlanEntry>> {
    let entries = rename_plan::read(plan)?;
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
    Ok(entries)
}
