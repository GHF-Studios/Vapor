//! Shared, evidence-backed onboarding journeys for Vapor frontends.

use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestEvidenceKind {
    InstalledComposition,
    SuccessfulOperation(&'static str),
    DeveloperToolchainReady,
    UserConfirmation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuestDefinition {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub points: u32,
    pub prerequisites: &'static [&'static str],
    pub evidence: QuestEvidenceKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JourneyDefinition {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub quests: &'static [QuestDefinition],
}

const PLAYER_QUESTS: &[QuestDefinition] = &[
    QuestDefinition {
        id: "player.install-composition",
        title: "Install a composition",
        description: "Install a playable Packagepack and its exact content graph.",
        points: 50,
        prerequisites: &[],
        evidence: QuestEvidenceKind::InstalledComposition,
    },
    QuestDefinition {
        id: "player.launch-composition",
        title: "Launch your game",
        description: "Start an installed composition through Vapor.",
        points: 100,
        prerequisites: &["player.install-composition"],
        evidence: QuestEvidenceKind::SuccessfulOperation("composition.launch"),
    },
];

const DEVELOPER_QUESTS: &[QuestDefinition] = &[
    QuestDefinition {
        id: "developer.toolchain-ready",
        title: "Prepare the Developer toolchain",
        description: "Vapor reports the required managed Rust toolchain and target tools ready.",
        points: 100,
        prerequisites: &[],
        evidence: QuestEvidenceKind::DeveloperToolchainReady,
    },
    QuestDefinition {
        id: "developer.run-workspace",
        title: "Run a source project",
        description: "Run a workspace configuration through Vapor. The Wheel Salvage Demo is a guided example.",
        points: 150,
        prerequisites: &[],
        evidence: QuestEvidenceKind::SuccessfulOperation("workspace.run-configuration"),
    },
    QuestDefinition {
        id: "developer.review-content-boundary",
        title: "Know what Vapor owns",
        description: "Review the distinction between Vapor Home, authored workspaces, and mounted game content.",
        points: 25,
        prerequisites: &[],
        evidence: QuestEvidenceKind::UserConfirmation,
    },
];

const JOURNEYS: &[JourneyDefinition] = &[
    JourneyDefinition {
        id: "player-first-play",
        title: "First Play",
        description: "Install and launch a playable experience managed by Vapor.",
        quests: PLAYER_QUESTS,
    },
    JourneyDefinition {
        id: "developer-first-demo",
        title: "First Developer Demo",
        description: "Prepare the managed toolchain and run a real source workspace.",
        quests: DEVELOPER_QUESTS,
    },
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum QuestEvidence {
    InstalledComposition {
        composition_id: String,
    },
    SuccessfulOperation {
        action: String,
        operation_id: String,
    },
    DeveloperToolchainReady,
    UserConfirmed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestCompletion {
    pub completed_at_unix: u64,
    pub evidence: QuestEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
struct ProgressDocument {
    schema_version: u32,
    #[serde(default)]
    completed: BTreeMap<String, QuestCompletion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestAvailability {
    Completed(QuestCompletion),
    Available,
    Locked(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestSnapshot {
    pub definition: QuestDefinition,
    pub availability: QuestAvailability,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JourneySnapshot {
    pub definition: JourneyDefinition,
    pub quests: Vec<QuestSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JourneyProgressSnapshot {
    pub journeys: Vec<JourneySnapshot>,
    pub points: u32,
    pub completed_quests: usize,
    pub total_quests: usize,
}

/// Return the stable journey catalog consumed by GUI and CLI.
pub fn journey_catalog() -> &'static [JourneyDefinition] {
    JOURNEYS
}

/// Locate the first-party Wheel Salvage source demo inside the canonical
/// Vapor-Client checkout, when that Developer source is present.
pub fn wheel_salvage_workspace() -> Result<Option<PathBuf>, String> {
    let installation = vapor_core::VaporInstallation::discover()
        .map_err(|error| format!("could not locate Vapor Home for the demo: {error}"))?;
    let workspace = installation
        .sources_root()
        .join("Vapor-Client")
        .join("Vapor-Examples");
    if workspace.join("Workspace.vapor.toml").is_file() {
        Ok(Some(workspace))
    } else {
        Ok(None)
    }
}

/// Read the persisted journey state and calculate prerequisite availability.
pub fn journey_progress() -> Result<JourneyProgressSnapshot, String> {
    let document = read_progress()?;
    Ok(snapshot(document))
}

/// Save an explicit confirmation, but only for quests defined as confirmable.
pub fn confirm_journey_quest(quest_id: &str) -> Result<JourneyProgressSnapshot, String> {
    let definition =
        find_quest(quest_id).ok_or_else(|| format!("unknown journey quest '{quest_id}'"))?;
    if definition.evidence != QuestEvidenceKind::UserConfirmation {
        return Err(format!(
            "quest '{quest_id}' is completed from Vapor-observed evidence, not a manual confirmation"
        ));
    }
    record_completion(quest_id, QuestEvidence::UserConfirmed)
}

/// Record an installation already visible in the Client's durable snapshot.
pub fn observe_installed_composition(
    composition_id: &str,
) -> Result<JourneyProgressSnapshot, String> {
    if composition_id.trim().is_empty() {
        return Err("composition identity is empty".to_owned());
    }
    record_completion(
        "player.install-composition",
        QuestEvidence::InstalledComposition {
            composition_id: composition_id.to_owned(),
        },
    )
}

/// Record only supported successful Client operations. Acceptance or a running
/// operation is never sufficient evidence.
pub fn observe_successful_operation(
    action: &str,
    operation_id: &str,
    composition_id: Option<&str>,
) -> Result<Option<JourneyProgressSnapshot>, String> {
    if operation_id.trim().is_empty() {
        return Err("operation identity is empty".to_owned());
    }
    let quest_id = match action {
        "composition.install" => "player.install-composition",
        "composition.launch" => "player.launch-composition",
        "workspace.run-configuration" => "developer.run-workspace",
        _ => return Ok(None),
    };
    let mut document = read_progress()?;
    if action == "composition.launch"
        && !document
            .completed
            .contains_key("player.install-composition")
    {
        let composition_id = composition_id
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "successful launch did not identify its composition".to_owned())?;
        apply_completion(
            &mut document,
            "player.install-composition",
            QuestEvidence::InstalledComposition {
                composition_id: composition_id.to_owned(),
            },
        );
    }
    let evidence = QuestEvidence::SuccessfulOperation {
        action: action.to_owned(),
        operation_id: operation_id.to_owned(),
    };
    apply_completion(&mut document, quest_id, evidence);
    write_progress(&document)?;
    Ok(Some(snapshot(document)))
}

/// Complete Developer setup only when the current shared role snapshot proves
/// the managed toolchain is ready for use.
pub fn observe_developer_toolchain(
    role: &str,
    rust_ready: bool,
    target_ready: bool,
    linker_ready: bool,
) -> Result<Option<JourneyProgressSnapshot>, String> {
    if role != "developer" || !(rust_ready && target_ready && linker_ready) {
        return Ok(None);
    }
    Ok(Some(record_completion(
        "developer.toolchain-ready",
        QuestEvidence::DeveloperToolchainReady,
    )?))
}

fn find_quest(quest_id: &str) -> Option<QuestDefinition> {
    JOURNEYS
        .iter()
        .flat_map(|journey| journey.quests)
        .find(|quest| quest.id == quest_id)
        .copied()
}

fn record_completion(
    quest_id: &str,
    evidence: QuestEvidence,
) -> Result<JourneyProgressSnapshot, String> {
    let mut document = read_progress()?;
    apply_completion(&mut document, quest_id, evidence);
    write_progress(&document)?;
    Ok(snapshot(document))
}

fn apply_completion(document: &mut ProgressDocument, quest_id: &str, evidence: QuestEvidence) {
    let Some(definition) = find_quest(quest_id) else {
        return;
    };
    if document.completed.contains_key(quest_id) {
        return;
    }
    if definition
        .prerequisites
        .iter()
        .any(|prerequisite| !document.completed.contains_key(*prerequisite))
    {
        return;
    }
    document.completed.insert(
        quest_id.to_owned(),
        QuestCompletion {
            completed_at_unix: now_unix(),
            evidence,
        },
    );
}

fn snapshot(document: ProgressDocument) -> JourneyProgressSnapshot {
    let mut points = 0;
    let mut completed_quests = 0;
    let total_quests = JOURNEYS.iter().map(|journey| journey.quests.len()).sum();
    let journeys = JOURNEYS
        .iter()
        .copied()
        .map(|definition| {
            let quests = definition
                .quests
                .iter()
                .copied()
                .map(|quest| {
                    let availability = if let Some(completion) = document.completed.get(quest.id) {
                        points += quest.points;
                        completed_quests += 1;
                        QuestAvailability::Completed(completion.clone())
                    } else {
                        let unmet = quest
                            .prerequisites
                            .iter()
                            .filter(|prerequisite| !document.completed.contains_key(**prerequisite))
                            .map(|prerequisite| (*prerequisite).to_owned())
                            .collect::<Vec<_>>();
                        if unmet.is_empty() {
                            QuestAvailability::Available
                        } else {
                            QuestAvailability::Locked(unmet)
                        }
                    };
                    QuestSnapshot {
                        definition: quest,
                        availability,
                    }
                })
                .collect();
            JourneySnapshot { definition, quests }
        })
        .collect();
    JourneyProgressSnapshot {
        journeys,
        points,
        completed_quests,
        total_quests,
    }
}

fn progress_path() -> Result<PathBuf, String> {
    vapor_core::VaporInstallation::discover()
        .map(|installation| {
            installation
                .state_root()
                .join("journeys")
                .join("progress.json")
        })
        .map_err(|error| format!("could not locate Vapor Home for journey progress: {error}"))
}

fn read_progress() -> Result<ProgressDocument, String> {
    let path = progress_path()?;
    if !path.exists() {
        return Ok(ProgressDocument {
            schema_version: SCHEMA_VERSION,
            ..ProgressDocument::default()
        });
    }
    let source = fs::read_to_string(&path).map_err(|error| {
        format!(
            "could not read journey progress '{}': {error}",
            path.display()
        )
    })?;
    let document: ProgressDocument = serde_json::from_str(&source)
        .map_err(|error| format!("journey progress '{}' is invalid: {error}", path.display()))?;
    if document.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "journey progress schema {} is not supported (expected {})",
            document.schema_version, SCHEMA_VERSION
        ));
    }
    Ok(document)
}

fn write_progress(document: &ProgressDocument) -> Result<(), String> {
    let path = progress_path()?;
    let parent = path
        .parent()
        .ok_or_else(|| "journey progress path has no parent".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "could not create journey state '{}': {error}",
            parent.display()
        )
    })?;
    let temporary = parent.join(format!(".progress-{}.tmp", std::process::id()));
    let encoded = serde_json::to_vec_pretty(document)
        .map_err(|error| format!("could not encode journey progress: {error}"))?;
    fs::write(&temporary, encoded)
        .map_err(|error| format!("could not write journey progress: {error}"))?;
    if let Err(error) = fs::rename(&temporary, &path) {
        if !cfg!(windows) || !path.exists() {
            return Err(format!("could not finalize journey progress: {error}"));
        }
        // Windows rename does not replace an existing destination. Move the old
        // record aside only after the new file is complete, and restore it if
        // promoting the staged file fails.
        let previous = parent.join(format!(".progress-{}.previous", std::process::id()));
        if previous.exists() {
            fs::remove_file(&previous).map_err(|remove_error| {
                format!("could not clear previous journey state: {remove_error}")
            })?;
        }
        fs::rename(&path, &previous)
            .map_err(|move_error| format!("could not stage old journey state: {move_error}"))?;
        if let Err(promote_error) = fs::rename(&temporary, &path) {
            let _ = fs::rename(&previous, &path);
            return Err(format!(
                "could not finalize journey progress: {promote_error}"
            ));
        }
        let _ = fs::remove_file(previous);
    }
    Ok(())
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}
