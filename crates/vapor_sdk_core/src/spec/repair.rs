//! SDK repair command specifications.

use super::{CommandSpec, StateSurface, spec};
use crate::repair::RepairCommand;

pub(super) fn describe(command: &RepairCommand) -> CommandSpec {
    match command {
        RepairCommand::Status => spec(
            "sdk repair status",
            "Inspect repairable SDK targets without proposing mutation.",
            StateSurface::ReadOnly,
            &[],
            &["display repair target health"],
        ),
        RepairCommand::Plan { .. } => spec(
            "sdk repair plan",
            "Prepare a repair plan without applying it.",
            StateSurface::RepairPlan,
            &["repair target is known"],
            &["compute proposed repair operations"],
        ),
        RepairCommand::Apply { .. } => spec(
            "sdk repair apply",
            "Apply repairs for an SDK target.",
            StateSurface::RepairApply,
            &["repair target is known", "planned mutations are acceptable"],
            &["repair selected SDK-managed state"],
        ),
    }
}
