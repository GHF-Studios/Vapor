//! Resolved installation context shared by the client and service host.

use std::{
    collections::HashMap,
    ffi::OsString,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostPlatform {
    Linux,
    Windows,
    MacOS,
    Other,
}

impl HostPlatform {
    pub fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOS
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Other
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientHostArguments {
    pub application_executable: PathBuf,
    pub workspace_root: Option<PathBuf>,
    pub state_directory: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientHostContext {
    pub application_executable: PathBuf,
    pub app_root: Option<PathBuf>,
    pub workspace_root: Option<PathBuf>,
    pub state_directory: PathBuf,
    pub platform: HostPlatform,
    pub steam_client_available: bool,
}

impl ClientHostContext {
    pub fn resolve(
        args: ClientHostArguments,
        inherited_environment: &HashMap<OsString, OsString>,
        platform: HostPlatform,
    ) -> Result<Self, String> {
        let workspace_root = args.workspace_root.or_else(|| {
            env_value(inherited_environment, "VAPOR_CLIENT_WORKSPACE")
                .or_else(|| env_value(inherited_environment, "VAPOR_STUDIO_WORKSPACE"))
                .map(PathBuf::from)
        });
        let state_directory = args
            .state_directory
            .or_else(|| {
                env_value(inherited_environment, "VAPOR_CLIENT_STATE_DIR")
                    .or_else(|| env_value(inherited_environment, "VAPOR_STUDIO_STATE_DIR"))
                    .map(PathBuf::from)
            })
            .or_else(|| {
                env_value(inherited_environment, "VAPOR_HOME")
                    .map(|root| PathBuf::from(root).join("data/state/studio"))
            })
            .ok_or_else(|| {
                "Vapor Client data path is unavailable; set VAPOR_HOME or VAPOR_CLIENT_STATE_DIR"
                    .to_owned()
            })?;
        let app_root = args
            .application_executable
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf);
        let steam_client_available = [
            "SteamAppId",
            "SteamGameId",
            "STEAM_COMPAT_CLIENT_INSTALL_PATH",
            "SteamPath",
        ]
        .iter()
        .any(|name| env_value(inherited_environment, name).is_some());
        Ok(Self {
            application_executable: args.application_executable,
            app_root,
            workspace_root,
            state_directory,
            platform,
            steam_client_available,
        })
    }
}

fn env_value<'a>(environment: &'a HashMap<OsString, OsString>, key: &str) -> Option<&'a OsString> {
    environment.get(OsString::from(key).as_os_str())
}
