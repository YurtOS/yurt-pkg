#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub repo_id: String,
    pub priority: i64,
}

pub fn select_repo_for_package<'a>(
    candidates: impl Iterator<Item = &'a Candidate>,
) -> Option<&'a Candidate> {
    candidates.min_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| left.repo_id.cmp(&right.repo_id))
    })
}
