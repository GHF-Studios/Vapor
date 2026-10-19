//! Shared Rust runtime target vocabulary for packaging and distribution.

/// Resolve the host's Rust target triple used by runtime packaging.
pub fn host_runtime_target() -> String {
    let arch = std::env::consts::ARCH;
    match (arch, std::env::consts::OS, std::env::consts::FAMILY) {
        ("x86_64", "linux", _) => "x86_64-unknown-linux-gnu".to_owned(),
        ("aarch64", "linux", _) => "aarch64-unknown-linux-gnu".to_owned(),
        ("x86_64", "windows", _) => "x86_64-pc-windows-gnullvm".to_owned(),
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

/// Reject target strings that could be interpreted as paths or shell syntax.
/// Validate a Rust runtime target triple accepted by Vapor's managed toolchain.
pub fn validate_runtime_target(target: &str) -> Result<(), String> {
    let valid = !target.is_empty()
        && target
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'));
    if valid {
        Ok(())
    } else {
        Err(format!(
            "runtime target must be a Rust target triple such as x86_64-pc-windows-gnullvm: {target}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{host_runtime_target, validate_runtime_target};

    #[test]
    fn host_mapping_always_produces_an_accepted_target() {
        assert!(validate_runtime_target(&host_runtime_target()).is_ok());
    }

    #[test]
    fn target_validation_rejects_path_and_shell_characters() {
        for target in ["../target", "x86_64;rm", "x86_64/windows", ""] {
            assert!(validate_runtime_target(target).is_err(), "{target}");
        }
    }
}
