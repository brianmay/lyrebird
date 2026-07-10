//! Validation checks run against a rename plan before anything is renamed:
//! duplicate sources/targets, no-op renames, missing sources, existing targets,
//! invalid filenames, and the ffprobe duration cross-check.
