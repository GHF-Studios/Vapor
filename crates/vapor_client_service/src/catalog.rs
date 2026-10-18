//! Complete catalog facts, scoped observation status, and independent search results.

use vapor_client_protocol::{
    CatalogEntry, CatalogQuery, CatalogSearchSnapshot, CatalogStatus, ContentReference,
};

const MAX_CATALOG_ITEMS: usize = 10_000;

#[derive(Default)]
pub(crate) struct CatalogState {
    entries: Vec<CatalogEntry>,
    query: CatalogQuery,
    status: CatalogStatus,
}

impl CatalogState {
    pub(crate) fn entries(&self) -> &[CatalogEntry] {
        &self.entries
    }

    pub(crate) fn status(&self) -> &CatalogStatus {
        &self.status
    }

    pub(crate) fn search(&self) -> CatalogSearchSnapshot {
        CatalogSearchSnapshot {
            query: self.query.as_str().to_owned(),
            matches: self
                .entries
                .iter()
                .filter(|entry| self.query.matches_entry(entry))
                .map(|entry| ContentReference {
                    id: entry.id.clone(),
                    version: entry.version.clone(),
                })
                .collect(),
        }
    }

    pub(crate) fn set_query(&mut self, query: &str) {
        self.query = CatalogQuery::new(query);
    }

    pub(crate) fn replace(&mut self, mut entries: Vec<CatalogEntry>) {
        let omitted = entries.len().saturating_sub(MAX_CATALOG_ITEMS);
        entries.truncate(MAX_CATALOG_ITEMS);
        self.entries = entries;
        self.status = CatalogStatus {
            loaded: true,
            error: None,
            omitted,
        };
    }

    pub(crate) fn fail(&mut self, error: String) {
        self.status.error = Some(error);
    }

    /// Different workspace facts cannot be presented as retained results for the new context.
    pub(crate) fn reset(&mut self) {
        self.entries.clear();
        self.status = CatalogStatus::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str) -> CatalogEntry {
        CatalogEntry {
            id: id.into(),
            version: Some("1.0".into()),
            kind: "game-mod".into(),
            published_file_id: Some("3762162251".into()),
            dependencies: vec![],
            conflicts: vec![],
            locations: vec![],
            role: vapor_client_protocol::CatalogItemRole::Other,
            actions: vec![],
        }
    }

    #[test]
    fn query_and_refresh_never_replace_facts_or_reset_the_query() {
        let mut catalog = CatalogState::default();
        catalog.replace(vec![entry("moon"), entry("mars")]);
        catalog.set_query("MOON");
        assert_eq!(catalog.entries().len(), 2);
        assert_eq!(catalog.search().matches.len(), 1);
        catalog.replace(vec![entry("moon"), entry("venus")]);
        assert_eq!(catalog.search().query, "moon");
        assert_eq!(catalog.search().matches[0].id, "moon");
        catalog.set_query("3762162251");
        assert_eq!(catalog.search().matches.len(), 2);
    }

    #[test]
    fn query_and_empty_results_do_not_clear_a_failed_observation() {
        let mut catalog = CatalogState::default();
        catalog.replace(vec![entry("moon")]);
        catalog.fail("provider unavailable".into());
        catalog.set_query("absent");
        assert!(catalog.search().matches.is_empty());
        assert_eq!(catalog.entries().len(), 1);
        assert!(catalog.status().loaded);
        assert!(!catalog.status().is_current());
        catalog.replace(vec![]);
        assert!(catalog.status().is_current());
        assert!(catalog.entries().is_empty());
    }

    #[test]
    fn truncation_and_context_reset_are_explicit() {
        let mut catalog = CatalogState::default();
        catalog.replace(
            (0..MAX_CATALOG_ITEMS + 2)
                .map(|i| entry(&i.to_string()))
                .collect(),
        );
        assert_eq!(catalog.entries().len(), MAX_CATALOG_ITEMS);
        assert_eq!(catalog.status().omitted, 2);
        assert!(!catalog.status().is_current());
        catalog.reset();
        catalog.fail("new context failed".into());
        assert!(catalog.entries().is_empty());
        assert!(!catalog.status().loaded);
    }
}
