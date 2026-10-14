use crate::steam::process::{steam_run_app_build, steam_run_app_build_plan};
use crate::steam::types::SteamRunAppBuildRequest;

use super::error::RootCommandError;
use super::package::root_package;
use super::types::{RootPackageRequest, RootPublishReport, RootPublishRequest};

/// Assemble the root package and publish it through SteamCMD.
pub fn root_publish(request: &RootPublishRequest) -> Result<RootPublishReport, RootCommandError> {
    let package_request = RootPackageRequest {
        plan: request.plan,
        app_id: request.app_id,
        depot_id: request.depot_id,
        description: request.description.clone(),
        set_live: request.set_live.clone(),
    };
    let package = root_package(&package_request)?;
    let steam_request = SteamRunAppBuildRequest {
        account: request.account.clone(),
        steamcmd: request.steamcmd.clone(),
        app_build_script: package.app_build_script.clone(),
    };

    if request.plan {
        let (steam_status, steam_args) = steam_run_app_build_plan(&steam_request)?;
        return Ok(RootPublishReport {
            planned: true,
            package,
            steamcmd: steam_status
                .resolved_steamcmd
                .unwrap_or_else(|| request.steamcmd.clone()),
            account: request.account.clone(),
            steam_args,
            status: None,
            set_live: request.set_live.clone(),
        });
    }

    let steam_report = steam_run_app_build(&steam_request)?;

    Ok(RootPublishReport {
        planned: false,
        package,
        steamcmd: steam_report
            .status
            .resolved_steamcmd
            .unwrap_or_else(|| request.steamcmd.clone()),
        account: request.account.clone(),
        steam_args: steam_report.steam_args,
        status: Some(steam_report.exit_status),
        set_live: request.set_live.clone(),
    })
}
