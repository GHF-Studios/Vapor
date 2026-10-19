use crate::discovery::ensure_contained;
use std::{fs, path::Path};

pub(crate) const WINDOWS_GNU_TARGET: &str = "x86_64-pc-windows-gnullvm";
const WINDOWS_RUNTIME_DLLS: &[&str] = &["libunwind.dll"];

pub(crate) fn copy_windows_runtime_dlls(
    root: &Path,
    target: &str,
    target_directory: &Path,
) -> Result<usize, String> {
    if target != WINDOWS_GNU_TARGET {
        return Ok(0);
    }
    ensure_contained(root, target_directory)?;
    fs::create_dir_all(target_directory).map_err(|error| {
        format!(
            "failed to create Windows runtime DLL directory '{}': {error}",
            target_directory.display()
        )
    })?;

    let source_directory = root.join("tools/llvm-mingw/x86_64-w64-mingw32/bin");
    let mut copied = 0;
    for dll in WINDOWS_RUNTIME_DLLS {
        let source = source_directory.join(dll);
        if !source.is_file() {
            return Err(format!(
                "cannot stage Windows runtime DLL '{dll}': missing {}\nhelp: run `rust-script --force <app-root>/resources/vapor/tools/production/app_setup/setup_development.rs` to install app-local llvm-mingw",
                source.display()
            ));
        }
        let target = target_directory.join(dll);
        ensure_contained(root, &target)?;
        fs::copy(&source, &target).map_err(|error| {
            format!(
                "failed to copy Windows runtime DLL '{}' to '{}': {error}",
                source.display(),
                target.display()
            )
        })?;
        copied += 1;
    }
    Ok(copied)
}
