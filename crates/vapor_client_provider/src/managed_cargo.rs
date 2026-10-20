use crate::discovery::EnvironmentPaths;
use std::{
    env,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::Command,
};
use vapor_core::RustToolchainCommandEnvironment;

/// Cargo from the installed Vapor app, with its pinned Rust environment.
///
/// Its output directory remains operation-owned: callers select the target
/// directory that matches the workspace and build purpose.
pub struct ApplicationCargo {
    cargo: PathBuf,
    environment: RustToolchainCommandEnvironment,
    path: OsString,
}

impl ApplicationCargo {
    /// Discover the installed app's Cargo toolchain for this operation.
    pub fn discover(paths: &EnvironmentPaths) -> Result<Self, String> {
        let installation = paths.installation();
        let cargo = installation
            .try_bundled_cargo()?
            .ok_or_else(|| "Vapor installation has no bundled Cargo executable".to_owned())?;
        let toolchain_bin = cargo
            .parent()
            .ok_or_else(|| format!("managed Cargo has no parent: {}", cargo.display()))?;
        let rustc = toolchain_bin.join(executable("rustc"));
        let rustdoc = toolchain_bin.join(executable("rustdoc"));
        if !rustc.is_file() || !rustdoc.is_file() {
            return Err(format!(
                "managed Rust toolchain is incomplete beside {}",
                cargo.display()
            ));
        }
        let toolchain = toolchain_bin
            .parent()
            .and_then(Path::file_name)
            .ok_or_else(|| "managed Rust toolchain has no stable directory name".to_owned())?
            .to_os_string();
        let root = installation.root();
        let environment = RustToolchainCommandEnvironment::new(
            root,
            root.join("cargo-home"),
            root.join("rustup-home"),
            toolchain,
            rustc,
            rustdoc,
        );
        let path = application_path(
            root,
            installation.binaries(),
            toolchain_bin,
            env::var_os("PATH").as_deref(),
        )?;

        Ok(Self {
            cargo,
            environment,
            path,
        })
    }

    /// The selected app-packaged Cargo executable.
    pub fn cargo_path(&self) -> &Path {
        &self.cargo
    }

    /// Create a child command with Vapor's selected app-packaged toolchain.
    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.cargo);
        self.environment.apply_to(&mut command);
        command.env("PATH", &self.path);
        command
    }
}

fn application_path(
    root: &Path,
    binaries: &Path,
    toolchain_bin: &Path,
    inherited: Option<&OsStr>,
) -> Result<OsString, String> {
    let mut entries = vec![
        toolchain_bin.to_path_buf(),
        binaries.to_path_buf(),
        root.join("bin"),
    ];
    if let Some(inherited) = inherited {
        entries
            .extend(env::split_paths(inherited).filter(|entry| !is_raw_app_tool_path(root, entry)));
    }
    env::join_paths(entries).map_err(|error| format!("failed to construct managed PATH: {error}"))
}

fn is_raw_app_tool_path(root: &Path, entry: &Path) -> bool {
    [
        root.join("cargo-home/bin"),
        root.join("rustup/bin"),
        root.join("tools/steamcmd"),
        root.join("tools/zig"),
        root.join("tools/cross/bin"),
        root.join("tools/llvm-mingw/bin"),
    ]
    .iter()
    .any(|raw_path| entry == raw_path)
        || (entry.starts_with(root.join("rustup-home/toolchains"))
            && entry.file_name().is_some_and(|name| name == "bin"))
}

fn executable(name: &str) -> String {
    format!("{name}{}", env::consts::EXE_SUFFIX)
}

#[cfg(test)]
mod tests {
    use std::{env, path::Path};

    #[test]
    fn application_path_keeps_managed_tools_ahead_and_filters_raw_tool_dirs() {
        let inherited = env::join_paths([
            Path::new("/app/tools/zig"),
            Path::new("/usr/bin"),
            Path::new("/app/rustup-home/toolchains/old/bin"),
            Path::new("/app/tools/steamcmd"),
        ])
        .expect("compose inherited path");

        let path = super::application_path(
            Path::new("/app"),
            Path::new("/app/bin/x86_64-linux"),
            Path::new("/app/rustup-home/toolchains/1.97.0/bin"),
            Some(&inherited),
        )
        .expect("construct app-managed path");
        let entries = env::split_paths(&path).collect::<Vec<_>>();

        assert_eq!(
            entries[0],
            Path::new("/app/rustup-home/toolchains/1.97.0/bin")
        );
        assert_eq!(entries[1], Path::new("/app/bin/x86_64-linux"));
        assert_eq!(entries[2], Path::new("/app/bin"));
        assert_eq!(entries[3], Path::new("/usr/bin"));
        assert_eq!(entries.len(), 4);
        assert!(
            !entries
                .iter()
                .any(|entry| entry == Path::new("/app/tools/zig"))
        );
    }
}
