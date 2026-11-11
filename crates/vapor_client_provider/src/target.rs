//! Shared Rust runtime target vocabulary for packaging and distribution.

#[cfg(test)]
use vapor_core::target::SUPPORTED_RUNTIME_TARGETS;
pub use vapor_core::target::{LINUX_GNU_TARGET, WINDOWS_GNU_TARGET};

/// Resolve the Rust compiler host triple for the installed toolchain.
pub fn host_rust_target() -> String {
    match (std::env::consts::ARCH, std::env::consts::OS) {
        ("x86_64", "windows") => "x86_64-pc-windows-msvc".to_owned(),
        ("aarch64", "windows") => "aarch64-pc-windows-msvc".to_owned(),
        ("x86_64", "linux") => "x86_64-unknown-linux-gnu".to_owned(),
        ("aarch64", "linux") => "aarch64-unknown-linux-gnu".to_owned(),
        ("x86_64", "macos") => "x86_64-apple-darwin".to_owned(),
        ("aarch64", "macos") => "aarch64-apple-darwin".to_owned(),
        _ => format!(
            "{}-{}-{}",
            std::env::consts::ARCH,
            std::env::consts::OS,
            std::env::consts::FAMILY
        ),
    }
}

/// Resolve the host's Rust target triple used by runtime packaging.
pub fn host_runtime_target() -> String {
    let arch = std::env::consts::ARCH;
    match (arch, std::env::consts::OS, std::env::consts::FAMILY) {
        ("x86_64", "linux", _) => LINUX_GNU_TARGET.to_owned(),
        ("aarch64", "linux", _) => "aarch64-unknown-linux-gnu".to_owned(),
        ("x86_64", "windows", _) => WINDOWS_GNU_TARGET.to_owned(),
        ("aarch64", "windows", _) => "aarch64-pc-windows-msvc".to_owned(),
        ("x86_64", "macos", _) => "x86_64-apple-darwin".to_owned(),
        ("aarch64", "macos", _) => "aarch64-apple-darwin".to_owned(),
        _ => format!(
            "{arch}-{}-{}",
            std::env::consts::OS,
            std::env::consts::FAMILY
        ),
    }
}

/// Validate a Rust runtime target triple accepted by Vapor's managed matrix.
pub fn validate_runtime_target(target: &str) -> Result<(), String> {
    vapor_core::target::validate_runtime_target(target)
}

#[cfg(test)]
mod tests {
    use super::{host_runtime_target, host_rust_target, validate_runtime_target};

    #[test]
    fn host_mapping_always_produces_an_accepted_target() {
        assert!(validate_runtime_target(&host_runtime_target()).is_ok());
        assert!(!host_rust_target().is_empty());
    }

    #[test]
    fn target_validation_accepts_only_the_managed_matrix() {
        for target in [
            "../target",
            "x86_64;rm",
            "x86_64/windows",
            "",
            "x86_64-pc-windows-msvc",
            "aarch64-unknown-linux-gnu",
        ] {
            assert!(validate_runtime_target(target).is_err(), "{target}");
        }
        for target in SUPPORTED_RUNTIME_TARGETS {
            assert!(validate_runtime_target(target).is_ok(), "{target}");
        }
    }
}
