//! Validation checks run against a rename plan before anything is renamed:
//! duplicate sources/targets, no-op renames, missing sources, existing targets,
//! invalid target paths, and the ffprobe duration cross-check.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use crate::ffprobe;
use crate::rename_plan::PlanEntry;

/// ±10% or ±30s, whichever is looser — published runtimes are rounded and
/// rips trim intros/credits differently.
const TOLERANCE_PCT: f64 = 10.0;
const TOLERANCE_MIN_SECS: f64 = 30.0;

const EXPECTED_EXTENSIONS: &[&str] = &["mkv", "mp4"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug)]
pub struct Issue {
    pub severity: Severity,
    pub line: u64,
    pub message: String,
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let severity = match self.severity {
            Severity::Error => "ERROR",
            Severity::Warning => "WARNING",
        };
        write!(f, "{severity} line {}: {}", self.line, self.message)
    }
}

/// Runs every check against the plan. `root` is the directory old/new paths
/// are relative to (the cwd in normal use). Filesystem is never modified.
pub fn validate(entries: &[PlanEntry], root: &Path) -> Vec<Issue> {
    let mut issues = Vec::new();
    let mut seen_old: HashMap<&str, u64> = HashMap::new();
    let mut seen_new: HashMap<&str, u64> = HashMap::new();

    for entry in entries {
        let plan = &entry.plan;
        let line = entry.line;
        let mut push = |severity, message| {
            issues.push(Issue {
                severity,
                line,
                message,
            })
        };

        if let Some(first) = seen_old.get(plan.old.as_str()) {
            push(
                Severity::Error,
                format!("source '{}' already appears on line {first}", plan.old),
            );
        } else {
            seen_old.insert(&plan.old, line);
        }

        if let Some(first) = seen_new.get(plan.new.as_str()) {
            push(
                Severity::Error,
                format!("target '{}' collides with line {first}", plan.new),
            );
        } else {
            seen_new.insert(&plan.new, line);
        }

        if plan.old == plan.new {
            push(
                Severity::Error,
                format!("source and target are identical ('{}')", plan.old),
            );
        }

        for (severity, message) in target_path_issues(&plan.new) {
            push(severity, message);
        }

        let source = root.join(&plan.old);
        let source_exists = source.is_file();
        if !source_exists {
            push(
                Severity::Error,
                format!("source file '{}' does not exist", plan.old),
            );
        }

        if plan.old != plan.new && root.join(&plan.new).exists() {
            push(
                Severity::Error,
                format!("target '{}' already exists", plan.new),
            );
        }

        if let (Some(expected), true) = (plan.expected_duration_secs, source_exists) {
            match ffprobe::duration_secs(&source) {
                Ok(actual) => {
                    let expected = expected as f64;
                    let allowed = allowed_tolerance_secs(expected);
                    let diff = (actual - expected).abs();
                    if diff > allowed {
                        push(
                            Severity::Error,
                            format!(
                                "'{}' duration {actual:.0}s differs from expected {expected:.0}s \
                                 by {diff:.0}s (allowed {allowed:.0}s) — possible mismatch with '{}'",
                                plan.old, plan.new
                            ),
                        );
                    }
                }
                Err(err) => push(
                    Severity::Warning,
                    format!("could not read duration of '{}': {err:#}", plan.old),
                ),
            }
        }
    }

    issues
}

fn allowed_tolerance_secs(expected: f64) -> f64 {
    (expected * TOLERANCE_PCT / 100.0).max(TOLERANCE_MIN_SECS)
}

fn target_path_issues(new: &str) -> Vec<(Severity, String)> {
    let mut issues = Vec::new();

    // Absolute targets are legitimate: resolve produces them when
    // LYREBIRD_TV_ROOT / LYREBIRD_MOVIE_ROOT are set.
    let relative = new.strip_prefix('/').unwrap_or(new);

    for component in relative.split('/') {
        if component.trim().is_empty() {
            issues.push((
                Severity::Error,
                format!("target '{new}' contains an empty path component"),
            ));
        } else if component == "." || component == ".." {
            issues.push((
                Severity::Error,
                format!("target '{new}' contains a '{component}' path component"),
            ));
        } else if component.contains(['<', '>', ':', '"', '\\', '|', '?', '*']) {
            issues.push((
                Severity::Warning,
                format!("target component '{component}' contains characters that are illegal on some filesystems"),
            ));
        }
    }

    let extension = Path::new(new).extension().and_then(|e| e.to_str());
    if !extension.is_some_and(|e| EXPECTED_EXTENSIONS.contains(&e.to_lowercase().as_str())) {
        issues.push((
            Severity::Warning,
            format!(
                "target '{new}' has an unexpected extension (expected one of {})",
                EXPECTED_EXTENSIONS.join(", ")
            ),
        ));
    }

    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rename_plan::RenamePlan;

    fn entry(line: u64, old: &str, new: &str) -> PlanEntry {
        PlanEntry {
            line,
            plan: RenamePlan {
                old: old.to_string(),
                new: new.to_string(),
                expected_duration_secs: None,
            },
        }
    }

    fn errors_of(issues: &[Issue]) -> Vec<&str> {
        issues
            .iter()
            .filter(|i| i.severity == Severity::Error)
            .map(|i| i.message.as_str())
            .collect()
    }

    #[test]
    fn tolerance_is_ten_percent_or_thirty_seconds() {
        assert_eq!(allowed_tolerance_secs(100.0), 30.0); // 10% would be too tight
        assert_eq!(allowed_tolerance_secs(1200.0), 120.0); // 10% is looser
    }

    #[test]
    fn detects_duplicates_and_noops() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.mkv"), b"").unwrap();

        let entries = [
            entry(1, "a.mkv", "New (2020)/New.mkv"),
            entry(2, "a.mkv", "Other (2020)/Other.mkv"), // duplicate source
            entry(3, "b.mkv", "New (2020)/New.mkv"),     // duplicate target, missing source
            entry(4, "a.mkv", "a.mkv"),                  // no-op (and dup source)
        ];
        let issues = validate(&entries, dir.path());
        let errors = errors_of(&issues);

        assert!(errors
            .iter()
            .any(|m| m.contains("already appears on line 1")));
        assert!(errors.iter().any(|m| m.contains("collides with line 1")));
        assert!(errors.iter().any(|m| m.contains("'b.mkv' does not exist")));
        assert!(errors
            .iter()
            .any(|m| m.contains("source and target are identical")));
    }

    #[test]
    fn detects_existing_target() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.mkv"), b"").unwrap();
        std::fs::create_dir(dir.path().join("Show (2020)")).unwrap();
        std::fs::write(dir.path().join("Show (2020)/taken.mkv"), b"").unwrap();

        let entries = [entry(1, "a.mkv", "Show (2020)/taken.mkv")];
        let issues = validate(&entries, dir.path());
        assert!(errors_of(&issues)
            .iter()
            .any(|m| m.contains("already exists")));
    }

    #[test]
    fn checks_target_path_shape() {
        let error_targets = ["a//b.mkv", "../up.mkv", "./here.mkv", "/media//tv/x.mkv"];
        for target in error_targets {
            assert!(
                target_path_issues(target)
                    .iter()
                    .any(|(s, _)| *s == Severity::Error),
                "expected error for {target}"
            );
        }

        let warning_targets = ["has|pipe.mkv", "wrong.avi"];
        for target in warning_targets {
            let issues = target_path_issues(target);
            assert!(
                issues.iter().all(|(s, _)| *s == Severity::Warning) && !issues.is_empty(),
                "expected warning-only for {target}"
            );
        }

        assert!(target_path_issues("Show (2020)/Season 01/Show - s01e01 - Pilot.mkv").is_empty());
        assert!(target_path_issues("Movie (1980)/Movie (1980).MP4").is_empty());
        // Absolute targets come from LYREBIRD_TV_ROOT / LYREBIRD_MOVIE_ROOT.
        assert!(target_path_issues("/media/tv/Show (2020)/Season 01/Ep.mkv").is_empty());
    }
}
