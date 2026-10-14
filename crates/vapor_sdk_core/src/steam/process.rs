use std::path::Path;
use std::process::{Command, ExitStatus};

use super::error::SteamCommandError;
use super::status::steam_status_for;
use super::types::{
    SteamLoginReport, SteamLoginRequest, SteamRunAppBuildReport, SteamRunAppBuildRequest,
    SteamStatusReport,
};

/// Run SteamCMD login and let SteamCMD handle password and Steam Guard prompts.
pub fn steam_login(request: &SteamLoginRequest) -> Result<SteamLoginReport, SteamCommandError> {
    validate_account(&request.account)?;

    let status = steam_status_for(&request.steamcmd)?;
    let steamcmd = status
        .resolved_steamcmd
        .clone()
        .ok_or_else(|| SteamCommandError::SteamCmdUnavailable(request.steamcmd.clone()))?;
    let steam_args = login_args(&request.account);
    let exit_status = run_steamcmd(&steamcmd, &steam_args)?;

    Ok(SteamLoginReport {
        status,
        account: request.account.clone(),
        steam_args,
        exit_status,
    })
}

/// Run a SteamPipe app build through SteamCMD.
pub fn steam_run_app_build(
    request: &SteamRunAppBuildRequest,
) -> Result<SteamRunAppBuildReport, SteamCommandError> {
    validate_account(&request.account)?;

    let status = steam_status_for(&request.steamcmd)?;
    let steamcmd = status
        .resolved_steamcmd
        .clone()
        .ok_or_else(|| SteamCommandError::SteamCmdUnavailable(request.steamcmd.clone()))?;
    let steam_args = run_app_build_args(&request.account, &request.app_build_script);
    let exit_status = run_steamcmd(&steamcmd, &steam_args)?;

    Ok(SteamRunAppBuildReport {
        status,
        account: request.account.clone(),
        steam_args,
        exit_status,
    })
}

/// Build the SteamCMD command line without executing it.
pub fn steam_run_app_build_plan(
    request: &SteamRunAppBuildRequest,
) -> Result<(SteamStatusReport, Vec<String>), SteamCommandError> {
    validate_account(&request.account)?;

    let status = steam_status_for(&request.steamcmd)?;
    let steam_args = run_app_build_args(&request.account, &request.app_build_script);
    Ok((status, steam_args))
}

fn validate_account(account: &str) -> Result<(), SteamCommandError> {
    if account.trim().is_empty() {
        return Err(SteamCommandError::MissingAccount);
    }

    Ok(())
}

fn login_args(account: &str) -> Vec<String> {
    vec!["+login".to_owned(), account.to_owned(), "+quit".to_owned()]
}

fn run_app_build_args(account: &str, app_build_script: &Path) -> Vec<String> {
    vec![
        "+login".to_owned(),
        account.to_owned(),
        "+run_app_build".to_owned(),
        app_build_script.display().to_string(),
        "+quit".to_owned(),
    ]
}

fn run_steamcmd(steamcmd: &Path, steam_args: &[String]) -> Result<ExitStatus, SteamCommandError> {
    Command::new(steamcmd)
        .args(steam_args)
        .status()
        .map_err(SteamCommandError::SteamCmdIo)
}
