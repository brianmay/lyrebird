//! Shells out to ffprobe to read a video file's actual duration.

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

pub fn duration_secs(path: &Path) -> Result<f64> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .context("could not run ffprobe (is it installed?)")?;

    if !output.status.success() {
        bail!(
            "ffprobe failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let duration = stdout.trim();
    duration.parse().with_context(|| {
        format!(
            "unexpected ffprobe output '{duration}' for {}",
            path.display()
        )
    })
}
