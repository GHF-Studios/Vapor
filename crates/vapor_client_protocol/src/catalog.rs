//! Catalog observation and text-query projections, separate from content authority.

use crate::{CatalogEntry, ContentReference};
use serde::{Deserialize, Serialize};

/// Completeness of the last catalog observation; request success cannot replace it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogStatus {
    pub loaded: bool,
    pub error: Option<String>,
    pub omitted: usize,
}

impl CatalogStatus {
    pub fn is_current(&self) -> bool {
        self.loaded && self.error.is_none() && self.omitted == 0
    }
}

/// Search results reference catalog facts instead of replacing them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogSearchSnapshot {
    pub query: String,
    pub matches: Vec<ContentReference>,
}

/// Case-insensitive literal substring matching used by catalog views in every client.
/// This does not decide content compatibility, installability, or launch readiness.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CatalogQuery(String);

impl CatalogQuery {
    pub fn new(query: &str) -> Self {
        Self(query.trim().to_lowercase())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn matches_fields<'a>(&self, fields: impl IntoIterator<Item = &'a str>) -> bool {
        self.0.is_empty()
            || fields
                .into_iter()
                .any(|field| field.to_lowercase().contains(&self.0))
    }

    pub fn matches_entry(&self, entry: &CatalogEntry) -> bool {
        self.matches_fields([
            entry.id.as_str(),
            entry.name.as_deref().unwrap_or_default(),
            entry.kind.as_str(),
            entry.version.as_deref().unwrap_or_default(),
            entry.published_file_id.as_deref().unwrap_or_default(),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_catalog_search_fields_share_one_normalized_predicate() {
        let entry = CatalogEntry {
            id: "author/moon".into(),
            name: Some("Moon".into()),
            kind: "game-mod".into(),
            version: Some("1.2.3".into()),
            published_file_id: Some("3762162251".into()),
            dependencies: vec![],
            dependency_details: vec![],
            conflicts: vec![],
            locations: vec![],
            role: crate::CatalogItemRole::Other,
            actions: vec![],
        };
        for query in [" MOON ", "GAME-MOD", "1.2.3", "3762162251", ""] {
            assert!(CatalogQuery::new(query).matches_entry(&entry), "{query}");
        }
        assert!(!CatalogQuery::new("absent").matches_entry(&entry));
        assert!(!CatalogQuery::new("moon game-mod").matches_entry(&entry));
    }

    #[test]
    fn retained_or_truncated_data_is_not_current_proof() {
        assert!(!CatalogStatus::default().is_current());
        let mut status = CatalogStatus {
            loaded: true,
            error: None,
            omitted: 0,
        };
        assert!(status.is_current());
        status.error = Some("offline".into());
        assert!(!status.is_current());
        status.error = None;
        status.omitted = 1;
        assert!(!status.is_current());
    }
}
