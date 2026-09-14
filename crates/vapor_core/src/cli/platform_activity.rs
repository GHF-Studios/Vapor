use super::commands::{PlatformServerRunKind, PlatformServerRunsArgs};
use super::{
    PLATFORM_SERVER_DEPLOY_WORKFLOW, PLATFORM_SERVER_GITHUB_REPOSITORY, command_output,
};
use serde::Deserialize;
use std::process::Command;

const PLATFORM_SERVER_CI_WORKFLOW: &str = "ci.yml";

#[derive(Debug, Deserialize)]
struct ActivityRun {
    #[serde(rename = "databaseId")]
    database_id: u64,

    #[serde(rename = "headSha")]
    head_sha: String,

    status: String,
    conclusion: Option<String>,

    #[serde(rename = "createdAt")]
    created_at: String,

    #[serde(rename = "displayTitle", default)]
    display_title: String,
}

#[derive(Debug)]
struct ActivityRecord {
    kind: PlatformServerRunKind,
    run: ActivityRun,
}

fn kind_label(kind: PlatformServerRunKind) -> &'static str {
    match kind {
        PlatformServerRunKind::All => "all",
        PlatformServerRunKind::Ci => "ci",
        PlatformServerRunKind::Deploy => "deploy",
    }
}

fn workflow(kind: PlatformServerRunKind) -> Result<&'static str, String> {
    match kind {
        PlatformServerRunKind::Ci => Ok(PLATFORM_SERVER_CI_WORKFLOW),
        PlatformServerRunKind::Deploy => Ok(PLATFORM_SERVER_DEPLOY_WORKFLOW),
        PlatformServerRunKind::All => Err("internal error: `all` is not one workflow".to_owned()),
    }
}

fn run_state(run: &ActivityRun) -> &str {
    run.conclusion.as_deref().unwrap_or(&run.status)
}

fn is_active(run: &ActivityRun) -> bool {
    matches!(
        run.status.as_str(),
        "queued" | "in_progress" | "requested" | "waiting" | "pending"
    )
}

fn short_sha(sha: &str) -> &str {
    &sha[..12.min(sha.len())]
}

fn workflow_runs(
    kind: PlatformServerRunKind,
    limit: usize,
    status: Option<&str>,
) -> Result<Vec<ActivityRecord>, String> {
    let workflow = workflow(kind)?;
    let limit = limit.to_string();

    let mut command = Command::new("gh");
    command.args([
        "run",
        "list",
        "--repo",
        PLATFORM_SERVER_GITHUB_REPOSITORY,
        "--workflow",
        workflow,
        "--branch",
        "main",
        "--limit",
        &limit,
        "--json",
        "databaseId,headSha,status,conclusion,createdAt,displayTitle",
    ]);

    if let Some(status) = status {
        command.args(["--status", status]);
    }

    let source = command_output(
        command,
        &format!("GitHub CLI Platform Server {} run query", kind_label(kind)),
    )?;

    let runs: Vec<ActivityRun> = serde_json::from_str(&source)
        .map_err(|error| format!("GitHub CLI returned invalid run JSON: {error}"))?;

    Ok(runs
        .into_iter()
        .map(|run| ActivityRecord { kind, run })
        .collect())
}

fn activity_runs(
    kind: PlatformServerRunKind,
    limit_per_workflow: usize,
    status: Option<&str>,
) -> Result<Vec<ActivityRecord>, String> {
    let mut runs = Vec::new();

    match kind {
        PlatformServerRunKind::All => {
            runs.extend(workflow_runs(
                PlatformServerRunKind::Ci,
                limit_per_workflow,
                status,
            )?);
            runs.extend(workflow_runs(
                PlatformServerRunKind::Deploy,
                limit_per_workflow,
                status,
            )?);
        }
        PlatformServerRunKind::Ci | PlatformServerRunKind::Deploy => {
            runs.extend(workflow_runs(kind, limit_per_workflow, status)?);
        }
    }

    runs.sort_by(|left, right| right.run.created_at.cmp(&left.run.created_at));
    Ok(runs)
}

pub(super) fn print_platform_server_activity_summary() {
    println!("GitHub activity:");

    match activity_runs(PlatformServerRunKind::All, 8, None) {
        Ok(runs) => {
            for kind in [PlatformServerRunKind::Ci, PlatformServerRunKind::Deploy] {
                let active = runs
                    .iter()
                    .find(|record| record.kind == kind && is_active(&record.run));
                let latest = runs.iter().find(|record| record.kind == kind);

                if let Some(record) = active.or(latest) {
                    let marker = if is_active(&record.run) {
                        "ACTIVE"
                    } else {
                        "latest"
                    };

                    println!(
                        "  {:<6}: {:<6} {:<11} {} {}",
                        kind_label(kind),
                        marker,
                        run_state(&record.run),
                        short_sha(&record.run.head_sha),
                        record.run.created_at,
                    );
                } else {
                    println!("  {:<6}: no runs", kind_label(kind));
                }
            }

            println!("  history: vapor platform-server runs");
        }
        Err(error) => println!("  unavailable: {error}"),
    }
}

pub(super) fn platform_server_runs(args: PlatformServerRunsArgs) -> Result<(), String> {
    if args.page == 0 {
        return Err("--page must be at least 1".to_owned());
    }
    if !(1..=50).contains(&args.per_page) {
        return Err("--per-page must be between 1 and 50".to_owned());
    }
    if args.page > 100 {
        return Err("--page must not exceed 100".to_owned());
    }

    let requested = args
        .page
        .checked_mul(args.per_page)
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| "requested run page is too large".to_owned())?;

    let runs = activity_runs(args.kind, requested, args.status.as_deref())?;
    let start = (args.page - 1) * args.per_page;

    println!(
        "Vapor Platform Server runs — page {}, {} per page{}:",
        args.page,
        args.per_page,
        args.status
            .as_deref()
            .map(|status| format!(", status={status}"))
            .unwrap_or_default(),
    );

    if start >= runs.len() {
        println!("  no runs on this page");
        return Ok(());
    }

    let end = (start + args.per_page).min(runs.len());
    println!("           ID  KIND    STATE        REVISION      CREATED                   TITLE");

    for record in &runs[start..end] {
        let title = if record.run.display_title.is_empty() {
            "(untitled)"
        } else {
            &record.run.display_title
        };

        println!(
            "  {:>11}  {:<6}  {:<11}  {}  {}  {}",
            record.run.database_id,
            kind_label(record.kind),
            run_state(&record.run),
            short_sha(&record.run.head_sha),
            record.run.created_at,
            title,
        );
    }

    if runs.len() > end {
        println!();
        println!(
            "Next page: vapor platform-server runs --kind {} --page {} --per-page {}{}",
            kind_label(args.kind),
            args.page + 1,
            args.per_page,
            args.status
                .as_deref()
                .map(|status| format!(" --status {status}"))
                .unwrap_or_default(),
        );
    }

    Ok(())
}
