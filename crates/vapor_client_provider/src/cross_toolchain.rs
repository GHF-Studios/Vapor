use crate::discovery::ensure_contained;
use std::{
    env, fs,
    path::{Path, PathBuf},
};

pub(crate) const LINUX_GNU_TARGET: &str = "x86_64-unknown-linux-gnu";
pub(crate) const WINDOWS_GNU_TARGET: &str = "x86_64-pc-windows-gnullvm";

const ZIG_LINKER_TARGETS: &[(&str, &str)] = &[(LINUX_GNU_TARGET, "x86_64-linux-gnu")];
const WINDOWS_RUNTIME_DLLS: &[&str] = &["libunwind.dll"];

#[derive(Debug, Clone)]
pub(crate) struct CrossToolchainStatus {
    pub(crate) installed: bool,
    pub(crate) path: PathBuf,
    pub(crate) missing: Vec<String>,
}

pub(crate) fn inspect(root: &Path) -> CrossToolchainStatus {
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
        let path = llvm_mingw_bin(root).join(tool);
        if !is_executable(&path) {
            missing.push(format!(
                "llvm-mingw tool {} (expected at {})",
                tool,
                path.display()
            ));
        }
    }
    for (target, _) in ZIG_LINKER_TARGETS {
        let linker = zig_linker_path(root, target);
        if !is_executable(&linker) {
            missing.push(format!(
                "{} linker wrapper (expected at {})",
                target,
                linker.display()
            ));
        }
    }
    CrossToolchainStatus {
        installed: missing.is_empty(),
        path: zig,
        missing,
    }
}

pub(crate) fn configure_linker_env(
    command: &mut std::process::Command,
    root: &Path,
    target: &str,
) -> Result<(), String> {
    if target == WINDOWS_GNU_TARGET {
        require_ready(root, target)?;
        let compiler = llvm_mingw_clang(root);
        command
            .env(cargo_linker_env(target), &compiler)
            .env(cargo_compiler_env(target), &compiler)
            .env(
                cargo_archiver_env(target),
                llvm_mingw_bin(root).join("x86_64-w64-mingw32-ar"),
            );
    } else if needs_zig_linker(target) {
        require_ready(root, target)?;
        command.env(cargo_linker_env(target), zig_linker_path(root, target));
    }
    Ok(())
}

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

    let source_directory = llvm_mingw_target_bin(root);
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

pub(crate) fn zig_executable(root: &Path) -> PathBuf {
    root.join("tools/zig").join(executable("zig"))
}

pub(crate) fn llvm_mingw_root(root: &Path) -> PathBuf {
    root.join("tools/llvm-mingw")
}

pub(crate) fn llvm_mingw_bin(root: &Path) -> PathBuf {
    llvm_mingw_root(root).join("bin")
}

fn llvm_mingw_target_bin(root: &Path) -> PathBuf {
    llvm_mingw_root(root).join("x86_64-w64-mingw32/bin")
}

fn llvm_mingw_clang(root: &Path) -> PathBuf {
    llvm_mingw_bin(root).join(executable("x86_64-w64-mingw32-clang"))
}

fn needs_zig_linker(target: &str) -> bool {
    target == LINUX_GNU_TARGET && host_runtime_target() != target
}

fn zig_linker_path(root: &Path, target: &str) -> PathBuf {
    root.join("tools/cross/bin")
        .join(linker_script_name(target))
}

fn require_ready(root: &Path, target: &str) -> Result<(), String> {
    let status = inspect(root);
    if status.installed {
        return Ok(());
    }
    Err(format!(
        "cannot build target {target}: app-local cross toolchains are missing\nmissing entries:\n  - {}\nhelp: run `rust-script --force <app-root>/resources/vapor/tools/production/app_setup/setup_development.rs`\nnote: cross-target builds use portable toolchains from the Steam app root",
        status.missing.join("\n  - ")
    ))
}

fn linker_script_name(target: &str) -> String {
    if cfg!(windows) {
        format!("{target}-zig-cc.cmd")
    } else {
        format!("{target}-zig-cc")
    }
}

fn cargo_linker_env(target: &str) -> String {
    format!(
        "CARGO_TARGET_{}_LINKER",
        target.replace('-', "_").to_ascii_uppercase()
    )
}

fn cargo_compiler_env(target: &str) -> String {
    format!("CC_{target}")
}

fn cargo_archiver_env(target: &str) -> String {
    format!("AR_{target}")
}

fn host_runtime_target() -> String {
    let arch = std::env::consts::ARCH;
    match (arch, std::env::consts::OS, std::env::consts::FAMILY) {
        ("x86_64", "linux", _) => LINUX_GNU_TARGET.to_owned(),
        ("x86_64", "windows", _) => WINDOWS_GNU_TARGET.to_owned(),
        ("aarch64", "linux", _) => "aarch64-unknown-linux-gnu".to_owned(),
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
