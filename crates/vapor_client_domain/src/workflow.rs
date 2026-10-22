use serde::{Deserialize, Serialize};

/// Progressive user-facing depth of a Vapor workflow.
///
/// A higher tier includes every lower tier. This classifies product work; it
/// is not an authorization token and does not represent tool installation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkflowTier {
    #[default]
    Player,
    Composer,
    Developer,
}

impl WorkflowTier {
    /// Whether this tier includes the work available at `required`.
    pub const fn includes(self, required: Self) -> bool {
        matches!(
            (self, required),
            (_, Self::Player)
                | (Self::Composer | Self::Developer, Self::Composer)
                | (Self::Developer, Self::Developer)
        )
    }
}

/// A locally managed tool or runtime resource needed by an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LocalCapability {
    ManagedRustToolchain,
    RustAnalyzer,
    TracyProfiler,
}

/// An external authority that must be checked when an operation is executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthorityRequirement {
    WorkshopPublisher,
    RootPublisher,
}

/// Static product classification and prerequisites for a client action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionProfile {
    pub workflow_tier: WorkflowTier,
    pub local_capabilities: &'static [LocalCapability],
    pub authority: &'static [AuthorityRequirement],
}

impl ActionProfile {
    pub const fn new(workflow_tier: WorkflowTier) -> Self {
        Self {
            workflow_tier,
            local_capabilities: &[],
            authority: &[],
        }
    }

    pub const fn with_local_capabilities(
        mut self,
        local_capabilities: &'static [LocalCapability],
    ) -> Self {
        self.local_capabilities = local_capabilities;
        self
    }

    pub const fn with_authority(mut self, authority: &'static [AuthorityRequirement]) -> Self {
        self.authority = authority;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::WorkflowTier;

    #[test]
    fn workflow_tiers_include_all_lower_levels() {
        assert!(WorkflowTier::Player.includes(WorkflowTier::Player));
        assert!(WorkflowTier::Composer.includes(WorkflowTier::Player));
        assert!(WorkflowTier::Composer.includes(WorkflowTier::Composer));
        assert!(WorkflowTier::Developer.includes(WorkflowTier::Player));
        assert!(WorkflowTier::Developer.includes(WorkflowTier::Composer));
        assert!(WorkflowTier::Developer.includes(WorkflowTier::Developer));
        assert!(!WorkflowTier::Player.includes(WorkflowTier::Composer));
        assert!(!WorkflowTier::Composer.includes(WorkflowTier::Developer));
    }
}
