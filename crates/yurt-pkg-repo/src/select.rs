#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub repo_id: String,
    /// Repository precedence. Lower values win; ties are broken
    /// deterministically by `repo_id`.
    pub priority: i64,
}

/// Select the repository that should supply a package when multiple
/// trusted repositories publish the same package name.
///
/// Lower numeric priority has higher precedence. Equal priorities fall
/// back to lexical `repo_id` order so selection remains stable.
pub fn select_repo_for_package<'a>(
    candidates: impl Iterator<Item = &'a Candidate>,
) -> Option<&'a Candidate> {
    candidates.min_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| left.repo_id.cmp(&right.repo_id))
    })
}
