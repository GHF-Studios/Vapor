//! Build and publication boundary for official distributed content.

/// Require the explicit Vapor release job context used by distribution CI.
///
/// The marker prevents an ordinary local Vapor process from accidentally
/// building a release profile or uploading content. Steam credentials must
/// still be scoped to the CI job; environment checks are not an auth boundary
/// against a machine owner who can set process variables.
pub fn require_distribution_job(operation: &str) -> Result<(), String> {
    let is_github_actions =
        std::env::var("GITHUB_ACTIONS").is_ok_and(|value| value.eq_ignore_ascii_case("true"));
    let is_ci =
        std::env::var("CI").is_ok_and(|value| value.eq_ignore_ascii_case("true") || value == "1");
    let has_release_marker =
        std::env::var("VAPOR_DISTRIBUTION_JOB").is_ok_and(|value| value == "steam-release-v1");

    if is_ci && is_github_actions && has_release_marker {
        Ok(())
    } else {
        Err(format!(
            "{operation} is restricted to Vapor's designated GitHub Actions distribution job"
        ))
    }
}
