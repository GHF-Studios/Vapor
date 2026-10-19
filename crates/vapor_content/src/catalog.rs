//! Provider-neutral exact content candidates for graph resolution.

use crate::{ContentKind, ContentVersionId, DependencySpec, VaporId};
use semver::VersionReq;
use std::collections::BTreeMap;

/// One exact content version available to a resolver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentCandidate {
    /// Exact semantic identity.
    pub identity: ContentVersionId,
    /// Canonical artifact role.
    pub kind: ContentKind,
    /// Local dependency binding to an authored content version range.
    pub dependencies: BTreeMap<String, DependencySpec>,
}

/// Provider-neutral set of exact content candidates.
///
/// Providers choose which candidates are eligible and attach provenance in
/// separate records. This type contains no paths, cache/install state, or
/// acquisition effects.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContentCandidates {
    entries: BTreeMap<ContentVersionId, ContentCandidate>,
}

impl ContentCandidates {
    /// Construct an empty candidate set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert one exact candidate, rejecting duplicate semantic versions.
    pub fn insert(&mut self, candidate: ContentCandidate) -> Result<(), DuplicateCandidate> {
        let identity = candidate.identity.clone();
        if self.entries.contains_key(&identity) {
            return Err(DuplicateCandidate(identity));
        }
        self.entries.insert(identity, candidate);
        Ok(())
    }

    /// Exact candidate lookup.
    pub fn get(&self, identity: &ContentVersionId) -> Option<&ContentCandidate> {
        self.entries.get(identity)
    }

    /// All exact versions of one stable content identity.
    pub fn versions<'a>(
        &'a self,
        id: &'a VaporId,
    ) -> impl Iterator<Item = &'a ContentCandidate> + 'a {
        self.entries
            .iter()
            .filter(move |(identity, _)| &identity.id == id)
            .map(|(_, candidate)| candidate)
    }

    /// Highest available version of one stable content identity.
    pub fn latest<'a>(&'a self, id: &'a VaporId) -> Option<&'a ContentCandidate> {
        self.versions(id)
            .max_by(|left, right| left.identity.version.cmp(&right.identity.version))
    }

    /// Highest available version satisfying one semantic version requirement.
    pub fn latest_matching<'a>(
        &'a self,
        id: &'a VaporId,
        requirement: &'a VersionReq,
    ) -> Option<&'a ContentCandidate> {
        self.versions(id)
            .filter(|candidate| requirement.matches(&candidate.identity.version))
            .max_by(|left, right| left.identity.version.cmp(&right.identity.version))
    }

    /// Iterate over candidates in stable exact-identity order.
    pub fn iter(&self) -> impl Iterator<Item = &ContentCandidate> {
        self.entries.values()
    }

    /// Number of exact candidates.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no exact candidates are available.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Duplicate exact candidate identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateCandidate(pub ContentVersionId);

impl std::fmt::Display for DuplicateCandidate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "duplicate content candidate `{}`", self.0)
    }
}

impl std::error::Error for DuplicateCandidate {}
