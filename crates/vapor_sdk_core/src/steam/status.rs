use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::root::types::ROOT_STEAM_APP_ID;
use crate::toolchain::toolchain_status;

use super::error::SteamCommandError;
use super::types::{SteamCmdSource, SteamStatusReport, SteamStatusRequest};

const STEAM_HOME_DIR: &str = "steam";
const STEAMCMD_STATE_DIR: &str = "steamcmd";

/// Inspect SteamCMD availability and Vapor's Steam state root.
pub fn steam_status(request: &SteamStatusRequest) -> Result<SteamStatusReport, SteamCommandError> {
    steam_status_for(&request.steamcmd)
}

pub(super) fn steam_status_for(steamcmd: &Path) -> Result<SteamStatusReport, SteamCommandError> {
    let toolchain = toolchain_status()?;
    let (resolved_steamcmd, steamcmd_source) = resolve_steamcmd(steamcmd);
    let steam_home = toolchain.vapor_home.join(STEAM_HOME_DIR);
    let steamcmd_state_root = steam_home.join(STEAMCMD_STATE_DIR);

    Ok(SteamStatusReport {
        app_id: ROOT_STEAM_APP_ID,
        vapor_home: toolchain.vapor_home,
        steam_home,
        steamcmd_state_root,
        requested_steamcmd: steamcmd.to_path_buf(),
        resolved_steamcmd,
        steamcmd_source,
        auth_model: "SteamCMD owns password prompts, Steam Guard, and saved login tokens.",
    })
}

fn resolve_steamcmd(steamcmd: &Path) -> (Option<PathBuf>, SteamCmdSource) {
    if steamcmd.components().count() > 1 || steamcmd.is_absolute() {
        if is_executable_file(steamcmd) {
            return (Some(steamcmd.to_path_buf()), SteamCmdSource::ExplicitPath);
        }
        return (None, SteamCmdSource::Unavailable);
    }

    let Some(path) = env::var_os("PATH") else {
        return (None, SteamCmdSource::Unavailable);
    };

    for root in env::split_paths(&path) {
        for candidate in executable_candidates(&root, steamcmd) {
            if is_executable_file(&candidate) {
                return (Some(candidate), SteamCmdSource::PathLookup);
            }
        }
    }

    (None, SteamCmdSource::Unavailable)
}

fn executable_candidates(root: &Path, command: &Path) -> Vec<PathBuf> {
    let candidate = root.join(command);
    let mut candidates = vec![candidate.clone()];

    if env::consts::EXE_SUFFIX.is_empty() {
        return candidates;
    }

    if candidate.extension().is_none() {
        let mut with_suffix: OsString = candidate.into_os_string();
        with_suffix.push(env::consts::EXE_SUFFIX);
        candidates.push(PathBuf::from(with_suffix));
    }

    candidates
}

fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}
