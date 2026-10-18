use serde::Deserialize;
use std::{fs::File, path::Path};
use vapor_client_protocol::{ClientAction, ClientResponseKind, OperationStatus, ServiceErrorCode};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub name: String,
    pub steps: Vec<ScenarioStep>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioStep {
    pub action: ClientAction,
    #[serde(default)]
    pub expect: ExpectedState,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedState {
    pub response: Option<ClientResponseKind>,
    pub error: Option<ServiceErrorCode>,
    pub active_composition_id: Option<String>,
    pub selected_content_id: Option<String>,
    pub catalog_len: Option<usize>,
    pub catalog_matches_len: Option<usize>,
    pub catalog_loaded: Option<bool>,
    pub catalog_has_error: Option<bool>,
    pub workspace_source_projects_len: Option<usize>,
    pub workspace_candidates_len: Option<usize>,
    pub operation_action: Option<String>,
    pub operation_status: Option<OperationStatus>,
    pub operation_message_contains: Option<String>,
}

pub fn load_scenario(path: &Path) -> Result<Scenario, String> {
    let file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let scenario: Scenario =
        serde_json::from_reader(file).map_err(|error| format!("{}: {error}", path.display()))?;
    if scenario.name.trim().is_empty() {
        return Err("scenario name cannot be empty".to_owned());
    }
    if scenario.steps.is_empty() {
        return Err("scenario must contain at least one step".to_owned());
    }
    Ok(scenario)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenario_schema_rejects_unknown_fields_and_empty_steps() {
        let empty = r#"{"name":"empty","steps":[]}"#;
        let scenario: Scenario = serde_json::from_str(empty).unwrap();
        assert!(scenario.steps.is_empty());
        assert!(load_scenario_from_text(empty).is_err());
        let unknown = r#"{"name":"bad","steps":[{"action":{"family":"catalog","action":{"action":"refresh"}},"production_provider":true}]}"#;
        assert!(load_scenario_from_text(unknown).is_err());
    }

    #[test]
    fn scenario_actions_are_typed_and_cannot_select_production_mode() {
        let json = r#"{"name":"fixture","steps":[{"action":{"family":"workshop","action":{"action":"publish","review_id":"review","confirmation":"token"}},"expect":{"response":"rejected","error":"action-unavailable"}}]}"#;
        let scenario = load_scenario_from_text(json).unwrap();
        assert!(matches!(
            scenario.steps[0].action,
            ClientAction::Workshop(_)
        ));
        assert_eq!(
            scenario.steps[0].expect.response,
            Some(ClientResponseKind::Rejected)
        );
        assert_eq!(
            scenario.steps[0].expect.error,
            Some(ServiceErrorCode::ActionUnavailable)
        );
    }

    fn load_scenario_from_text(text: &str) -> Result<Scenario, String> {
        let scenario: Scenario = serde_json::from_str(text).map_err(|error| error.to_string())?;
        if scenario.name.trim().is_empty() || scenario.steps.is_empty() {
            return Err("scenario must have a name and at least one step".to_owned());
        }
        Ok(scenario)
    }
}
