//! Canonical Rust module vocabulary shared by authoring tools and the service.

use serde::{Deserialize, Serialize};

/// A Rust module category Vapor knows how to scaffold.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalRustModuleKind {
    #[default]
    Components,
    Constants,
    Events,
    Functions,
    Resources,
    RunConditions,
    Statics,
    Systems,
    Types,
}

impl CanonicalRustModuleKind {
    /// Every module kind in its stable chooser order.
    pub const ALL: [Self; 9] = [
        Self::Components,
        Self::Constants,
        Self::Events,
        Self::Functions,
        Self::Resources,
        Self::RunConditions,
        Self::Statics,
        Self::Systems,
        Self::Types,
    ];

    /// The canonical source filename stem for this module kind.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Components => "components",
            Self::Constants => "constants",
            Self::Events => "events",
            Self::Functions => "functions",
            Self::Resources => "resources",
            Self::RunConditions => "run_conditions",
            Self::Statics => "statics",
            Self::Systems => "systems",
            Self::Types => "types",
        }
    }

    /// Starter module documentation, kept beside the vocabulary it describes.
    pub const fn documentation(self) -> &'static str {
        match self {
            Self::Components => "//! Components and component-owned data.\n",
            Self::Constants => "//! Shared compile-time values.\n",
            Self::Events => "//! Events exchanged between systems.\n",
            Self::Functions => "//! General-purpose functions.\n",
            Self::Resources => "//! World resources.\n",
            Self::RunConditions => "//! Conditions that control system execution.\n",
            Self::Statics => "//! Shared static values.\n",
            Self::Systems => "//! Bevy systems and system sets.\n",
            Self::Types => "//! Domain types that do not fit a narrower module.\n",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CanonicalRustModuleKind;

    #[test]
    fn canonical_module_names_and_documentation_are_unique() {
        let mut names = std::collections::BTreeSet::new();
        for module in CanonicalRustModuleKind::ALL {
            assert!(names.insert(module.name()));
            assert!(module.documentation().starts_with("//! "));
        }
        assert_eq!(names.len(), CanonicalRustModuleKind::ALL.len());
    }
}
