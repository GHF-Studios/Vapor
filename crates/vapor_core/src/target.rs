//! Canonical runtime target matrix shared by Vapor build and install code.

/// Supported Linux runtime target.
pub const LINUX_GNU_TARGET: &str = "x86_64-unknown-linux-gnu";
/// Supported Windows runtime target.
pub const WINDOWS_GNU_TARGET: &str = "x86_64-pc-windows-gnullvm";
/// The only runtime targets supported by Vapor's current build matrix.
pub const SUPPORTED_RUNTIME_TARGETS: &[&str] = &[LINUX_GNU_TARGET, WINDOWS_GNU_TARGET];

/// Validate a Rust runtime triple against Vapor's supported platform matrix.
pub fn validate_runtime_target(target: &str) -> Result<(), String> {
    if SUPPORTED_RUNTIME_TARGETS.contains(&target) {
        Ok(())
    } else {
        Err(format!(
            "unsupported Vapor runtime target `{target}`; supported targets are: {}",
            SUPPORTED_RUNTIME_TARGETS.join(", ")
        ))
    }
}
