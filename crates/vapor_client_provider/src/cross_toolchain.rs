use crate::{
    discovery::ensure_contained,
    target::host_runtime_target,
};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

pub use crate::target::{LINUX_GNU_TARGET, WINDOWS_GNU_TARGET};
const WINDOWS_RUNTIME_DLLS: &[&str] = &["libunwind.dll"];

/// Availability of Vapor's managed cross-compilation tools.
#[derive(Debug, Clone)]
pub struct CrossToolchainStatus {
    /// Whether every managed cross tool and linker wrapper is available.
    pub installed: bool,
    /// The Zig executable used by the Linux cross-linker wrapper.
    pub path: PathBuf,
    /// Missing tools, reported with their expected app-local paths.
    pub missing: Vec<String>,
}

/// Inspect the app-local cross-compilation toolchains.
pub fn inspect(root: &Path) -> CrossToolchainStatus {
    let zig = zig_executable(root);
    let mut missing = Vec::new();
    if !is_executable(&zig) {
        missing.push(format!("zig (expected at {})", zig.display()));
    }
    let clang = llvm_mingw_clang(root);
    if !is_executable(&clang) {
        missing.push(format!(
            "llvm-mingw clang (expected at {})",
            clang.display()
        ));
    }
    for tool in [
        "x86_64-w64-mingw32-dlltool",
        "llvm-dlltool",
        "x86_64-w64-mingw32-ar",
    ] {
        let path = llvm_mingw_bin(root).join(executable(tool));
        if !is_executable(&path) {
            missing.push(format!(
                "llvm-mingw tool {} (expected at {})",
                tool,
                path.display()
            ));
        }
    }
    let archiver = llvm_ar(root);
    if !is_executable(&archiver) {
        missing.push(format!(
            "LLVM archiver for {LINUX_GNU_TARGET} (expected at {})",
            archiver.display()
        ));
    }
    let compiler = linux_cc_compiler(root);
    if !is_executable(&compiler) {
        missing.push(format!(
            "Vapor Zig C compiler adapter for {LINUX_GNU_TARGET} (expected at {})",
            compiler.display()
        ));
    }
    CrossToolchainStatus {
        installed: missing.is_empty(),
        path: zig,
        missing,
    }
}

/// Configure the app-local linker, compiler, and archiver for a Cargo target.
///
/// Selects the Vapor-managed toolchain from the requested target, even when it
/// matches the host. This keeps host and cross builds on the same ABI/tool path.
/// Unsupported targets are rejected before Cargo is invoked.
pub fn configure_linker_env(
    command: &mut Command,
    root: &Path,
    target: &str,
) -> Result<(), String> {
    vapor_core::configure_runtime_target_linker_env(command, root, target)
        .map_err(|error| error.to_string())
}

/// Copy runtime DLLs required by a Windows GNU binary into its deploy directory.
pub fn copy_windows_runtime_dlls(
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
        if target.is_file() {
            let source_bytes = fs::read(&source).map_err(|error| {
                format!(
                    "failed to read Windows runtime DLL '{}': {error}",
                    source.display()
                )
            })?;
            let target_bytes = fs::read(&target).map_err(|error| {
                format!(
                    "failed to inspect deployed Windows runtime DLL '{}': {error}",
                    target.display()
                )
            })?;
            if source_bytes == target_bytes {
                continue;
            }
        }
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

fn zig_executable(root: &Path) -> PathBuf {
    root.join("tools/zig").join(executable("zig"))
}

fn llvm_mingw_bin(root: &Path) -> PathBuf {
    root.join("tools/llvm-mingw/bin")
}

fn llvm_mingw_clang(root: &Path) -> PathBuf {
    llvm_mingw_bin(root).join(executable("x86_64-w64-mingw32-clang"))
}

fn llvm_ar(root: &Path) -> PathBuf {
    llvm_mingw_bin(root).join(executable("llvm-ar"))
}

fn linux_cc_compiler(root: &Path) -> PathBuf {
    root.join("bin")
        .join(host_runtime_target())
        .join(executable("vapor-linux-cc"))
}

fn executable(name: &str) -> String {
    format!("{name}{}", env::consts::EXE_SUFFIX)
}

fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{WINDOWS_GNU_TARGET, configure_linker_env};
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "vapor-cross-toolchain-test-{}-{}",
                std::process::id(),
                std::thread::current().name().unwrap_or("test")
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("tools/llvm-mingw/bin")).unwrap();
            Self(root)
        }

        fn executable(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            fs::write(&path, b"test tool").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut permissions = fs::metadata(&path).unwrap().permissions();
                permissions.set_mode(0o755);
                fs::set_permissions(&path, permissions).unwrap();
            }
            path
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn windows_gnu_target_uses_the_same_mingw_compiler_and_archiver() {
        let root = TestRoot::new();
        let compiler = root.executable("tools/llvm-mingw/bin/x86_64-w64-mingw32-clang");
        let archiver = root.executable("tools/llvm-mingw/bin/x86_64-w64-mingw32-ar");
        let mut command = vapor_core::managed_process::background_command("cargo");

        configure_linker_env(&mut command, &root.0, WINDOWS_GNU_TARGET).unwrap();

        let environment = command
            .get_envs()
            .filter_map(|(name, value)| {
                Some((
                    name.to_string_lossy().into_owned(),
                    Path::new(value?).to_path_buf(),
                ))
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(
            environment.get(&format!(
                "CARGO_TARGET_{}_LINKER",
                WINDOWS_GNU_TARGET.replace('-', "_").to_ascii_uppercase()
            )),
            Some(&compiler)
        );
        assert_eq!(
            environment.get(&format!("CC_{WINDOWS_GNU_TARGET}")),
            Some(&compiler)
        );
        assert_eq!(
            environment.get(&format!("AR_{WINDOWS_GNU_TARGET}")),
            Some(&archiver)
        );
    }
}
