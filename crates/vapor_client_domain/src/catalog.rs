use crate::ContentReference;
use std::{collections::BTreeMap, path::PathBuf};

/// Source that currently makes one exact content version available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogLocationKind {
    /// Authored in the active workspace.
    Workspace,
    /// Downloaded into local Library storage.
    Cache,
    /// Materialized in the active installation.
    Installed,
}

/// Provider observation for one local copy of a content version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogLocation {
    /// Provider that reported this copy.
    pub kind: CatalogLocationKind,
    /// Local directory for the provider copy.
    pub root: PathBuf,
    /// Whether this copy is currently available for selection.
    pub enabled: bool,
    /// Content tree fingerprint recorded for cache or installation.
    pub fingerprint: Option<String>,
    /// Original source recorded when this installed copy was created.
    pub install_source: Option<String>,
}

/// A same-version identity whose locally observed metadata disagrees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogConflict {
    /// Metadata field that disagreed.
    pub field: String,
    /// Value from the first observation.
    pub first_value: String,
    /// Location that supplied the first value.
    pub first_location: PathBuf,
    /// Value from the conflicting observation.
    pub conflicting_value: String,
    /// Location that supplied the conflicting value.
    pub conflicting_location: PathBuf,
}

/// One exact content version reconciled across available providers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogContent {
    /// Stable, version-independent content identity.
    pub id: String,
    /// Human-readable name authored by the content publisher, when present.
    pub name: Option<String>,
    /// Exact semantic version, when authored.
    pub version: Option<String>,
    /// Canonical Rust-defined content kind.
    pub kind: String,
    /// Associated Workshop item, when known.
    pub published_file_id: Option<String>,
    /// Authored dependency edges retained as structured identity and policy.
    pub dependencies: Vec<CatalogDependency>,
    /// Provider observations for this exact identity.
    pub locations: Vec<CatalogLocation>,
    /// Conflicts make this identity unsafe to select or publish until resolved.
    pub conflicts: Vec<CatalogConflict>,
}

/// One authored dependency edge for a catalog item.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CatalogDependency {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub relationship: String,
    #[serde(default)]
    pub optional: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workshop_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl std::fmt::Display for CatalogDependency {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.id)?;
        if let Some(version) = &self.version {
            write!(formatter, " {version}")?;
        }
        if self.optional {
            write!(formatter, " (optional)")?;
        }
        Ok(())
    }
}

impl CatalogContent {
    pub fn identity(&self) -> ContentReference {
        ContentReference {
            id: self.id.clone(),
            version: self.version.clone(),
        }
    }

    pub fn is_unambiguous(&self) -> bool {
        self.conflicts.is_empty()
    }
}

/// Merge local observations by exact identity and record metadata disagreement.
///
/// Providers supply facts and locations; this shared rule decides whether two
/// observations describe the same usable content version.
pub fn merge_catalog_entries(
    incoming: impl IntoIterator<Item = CatalogContent>,
) -> Vec<CatalogContent> {
    let mut entries = BTreeMap::new();
    for incoming in incoming {
        merge_entry(&mut entries, incoming);
    }
    entries.into_values().collect()
}

fn merge_entry(
    entries: &mut BTreeMap<ContentReference, CatalogContent>,
    mut incoming: CatalogContent,
) {
    let key = incoming.identity();
    if let Some(existing) = entries.get_mut(&key) {
        let first_location = existing
            .locations
            .first()
            .map(|location| location.root.clone())
            .unwrap_or_default();
        let incoming_location = incoming
            .locations
            .first()
            .map(|location| location.root.clone())
            .unwrap_or_default();
        let mut conflicts = Vec::new();
        if existing.kind != incoming.kind {
            conflicts.push(("kind", existing.kind.clone(), incoming.kind.clone()));
        }
        if let (Some(first), Some(next)) = (&existing.name, &incoming.name)
            && first != next
        {
            conflicts.push(("name", first.clone(), next.clone()));
        }
        if let (Some(first), Some(next)) =
            (&existing.published_file_id, &incoming.published_file_id)
            && first != next
        {
            conflicts.push(("PublishedFileId", first.clone(), next.clone()));
        }
        let mut first_dependencies = existing.dependencies.clone();
        let mut next_dependencies = incoming.dependencies.clone();
        first_dependencies.sort();
        next_dependencies.sort();
        if first_dependencies != next_dependencies {
            conflicts.push((
                "dependencies",
                format_catalog_dependencies(&first_dependencies),
                format_catalog_dependencies(&next_dependencies),
            ));
        }
        // Cache and installed trees are different realizations: packaging
        // intentionally narrows runtime declarations and payloads to the host
        // target. Compare fingerprints only between observations from the same
        // provider class, where a mismatch really is a duplicate-copy conflict.
        let fingerprint_conflict = existing.locations.iter().find_map(|first| {
            let first_fingerprint = first.fingerprint.as_ref()?;
            incoming
                .locations
                .iter()
                .find(|next| {
                    next.kind == first.kind
                        && next
                            .fingerprint
                            .as_ref()
                            .is_some_and(|next_fingerprint| next_fingerprint != first_fingerprint)
                })
                .and_then(|next| next.fingerprint.as_ref())
                .map(|next_fingerprint| (first_fingerprint.clone(), next_fingerprint.clone()))
        });
        if let Some((first, next)) = fingerprint_conflict {
            conflicts.push(("content fingerprint", first, next));
        }
        existing.conflicts.extend(conflicts.into_iter().map(
            |(field, first_value, conflicting_value)| CatalogConflict {
                field: field.to_owned(),
                first_value,
                first_location: first_location.clone(),
                conflicting_value,
                conflicting_location: incoming_location.clone(),
            },
        ));
        existing.conflicts.append(&mut incoming.conflicts);
        if existing.published_file_id.is_none() {
            existing.published_file_id = incoming.published_file_id.take();
        }
        if existing.name.is_none() {
            existing.name = incoming.name.take();
        }
        if existing.dependencies.is_empty() {
            existing.dependencies = std::mem::take(&mut incoming.dependencies);
        }
        for location in incoming.locations {
            if !existing.locations.contains(&location) {
                existing.locations.push(location);
            }
        }
    } else {
        entries.insert(key, incoming);
    }
}

fn format_catalog_dependencies(dependencies: &[CatalogDependency]) -> String {
    dependencies
        .iter()
        .map(|dependency| {
            let version = dependency
                .version
                .as_deref()
                .map(|version| format!("@{version}"))
                .unwrap_or_default();
            let optional = if dependency.optional {
                " (optional)"
            } else {
                ""
            };
            format!("{}{version}{optional}", dependency.id)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::{CatalogContent, CatalogLocation, CatalogLocationKind, merge_catalog_entries};
    use std::path::PathBuf;

    fn observation(kind: CatalogLocationKind, fingerprint: &str) -> CatalogContent {
        CatalogContent {
            id: "studio/example/game".to_owned(),
            name: Some("Example".to_owned()),
            version: Some("0.5.0".to_owned()),
            kind: "game".to_owned(),
            published_file_id: Some("123".to_owned()),
            dependencies: Vec::new(),
            locations: vec![CatalogLocation {
                kind,
                root: PathBuf::from(format!("/{kind:?}")),
                enabled: true,
                fingerprint: Some(fingerprint.to_owned()),
                install_source: None,
            }],
            conflicts: Vec::new(),
        }
    }

    #[test]
    fn target_specific_installation_fingerprint_does_not_conflict_with_full_cache() {
        let merged = merge_catalog_entries([
            observation(CatalogLocationKind::Cache, "sha256:all-targets"),
            observation(CatalogLocationKind::Installed, "sha256:host-target"),
        ]);

        assert_eq!(merged.len(), 1);
        assert!(merged[0].conflicts.is_empty());
        assert_eq!(merged[0].locations.len(), 2);
    }

    #[test]
    fn mismatched_fingerprints_for_same_provider_kind_remain_a_conflict() {
        let merged = merge_catalog_entries([
            observation(CatalogLocationKind::Cache, "sha256:first"),
            observation(CatalogLocationKind::Cache, "sha256:second"),
        ]);

        assert_eq!(merged[0].conflicts.len(), 1);
        assert_eq!(merged[0].conflicts[0].field, "content fingerprint");
    }

    #[test]
    fn later_same_provider_copy_is_checked_even_if_first_copy_matches() {
        let mut incoming = observation(CatalogLocationKind::Cache, "sha256:first");
        incoming.locations.push(CatalogLocation {
            kind: CatalogLocationKind::Cache,
            root: PathBuf::from("/Cache/second"),
            enabled: true,
            fingerprint: Some("sha256:second".to_owned()),
            install_source: None,
        });

        let merged = merge_catalog_entries([
            observation(CatalogLocationKind::Cache, "sha256:first"),
            incoming,
        ]);

        assert_eq!(merged[0].conflicts.len(), 1);
        assert_eq!(merged[0].conflicts[0].field, "content fingerprint");
    }
}
