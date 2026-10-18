use crate::ContentReference;
use std::{
    collections::BTreeMap,
    path::PathBuf,
};

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
    /// Exact semantic version, when authored.
    pub version: Option<String>,
    /// Canonical Rust-defined content kind.
    pub kind: String,
    /// Associated Workshop item, when known.
    pub published_file_id: Option<String>,
    /// Human-readable dependency descriptions from local manifests.
    pub dependencies: Vec<String>,
    /// Provider observations for this exact identity.
    pub locations: Vec<CatalogLocation>,
    /// Conflicts make this identity unsafe to select or publish until resolved.
    pub conflicts: Vec<CatalogConflict>,
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
                first_dependencies.join(", "),
                next_dependencies.join(", "),
            ));
        }
        let first_fingerprint = existing
            .locations
            .iter()
            .find_map(|location| location.fingerprint.as_ref());
        let incoming_fingerprint = incoming
            .locations
            .iter()
            .find_map(|location| location.fingerprint.as_ref());
        if let (Some(first), Some(next)) = (first_fingerprint, incoming_fingerprint)
            && first != next
        {
            conflicts.push(("content fingerprint", first.clone(), next.clone()));
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
