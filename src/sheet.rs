//! Stage 0: contact-sheet generation — a grid of thumbnails per file so rips
//! can be identified at a glance instead of watched.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::ffprobe;

const TILE_COLS: u32 = 4;
const TILE_ROWS: u32 = 4;
const TILE_WIDTH: u32 = 320;

pub fn generate(input: &Path) -> Result<PathBuf> {
    let duration = ffprobe::duration_secs(input)?;
    if duration <= 0.0 {
        bail!("{} has zero duration", input.display());
    }

    // Sample exactly one grid's worth of frames, spread evenly across the
    // runtime, regardless of the file's frame rate or length.
    let tiles = TILE_COLS * TILE_ROWS;
    let fps = f64::from(tiles) / duration;
    let filter = format!("fps={fps},scale={TILE_WIDTH}:-1,tile={TILE_COLS}x{TILE_ROWS}");

    let output_path = output_path(input);
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-i"])
        .arg(input)
        .args(["-vf", &filter, "-frames:v", "1", "-update", "1"])
        .arg(&output_path)
        .output()
        .context("could not run ffmpeg (is it installed?)")?;

    if !output.status.success() {
        bail!(
            "ffmpeg failed for {}: {}",
            input.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output_path)
}

/// `title_01.mkv` -> `title_01_sheet.png`, alongside the input.
fn output_path(input: &Path) -> PathBuf {
    let stem = input.file_stem().unwrap_or_default().to_string_lossy();
    input.with_file_name(format!("{stem}_sheet.png"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheet_sits_next_to_input() {
        assert_eq!(
            output_path(Path::new("rips/title 01.mkv")),
            Path::new("rips/title 01_sheet.png")
        );
    }
}
