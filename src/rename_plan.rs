//! The resolved rename plan: intermediate between `resolve` and `validate`/`apply`.

/// One planned rename, as serialized to/from `renames.txt`.
#[derive(Debug, Clone)]
pub struct RenamePlan {
    pub old: String,
    pub new: String,
    pub expected_duration_secs: Option<u64>,
}
