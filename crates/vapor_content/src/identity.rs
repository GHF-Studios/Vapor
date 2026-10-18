//! Semantic identity for Vapor Content.

use semver::Version;
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use std::str::FromStr;

/// Immutable, human-readable semantic identity of one Vapor Content artifact.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct VaporId(String);

impl VaporId {
    /// Construct an ID that is nonempty and contains no whitespace.
    pub fn new(value: impl Into<String>) -> Result<Self, ParseVaporIdError> {
        let value = value.into();
        if value.is_empty() || value.chars().any(char::is_whitespace) {
            return Err(ParseVaporIdError);
        }
        Ok(Self(value))
    }

    /// Borrow the canonical ID text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the ID and return its text.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for VaporId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for VaporId {
    type Err = ParseVaporIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for VaporId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// Invalid Vapor ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseVaporIdError;

impl fmt::Display for ParseVaporIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Vapor ID must be non-empty and contain no whitespace")
    }
}

impl std::error::Error for ParseVaporIdError {}

/// Exact identity of one version of Vapor Content.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ContentVersionId {
    /// Stable, version-independent content ID.
    pub id: VaporId,
    /// Exact semantic version.
    pub version: Version,
}

/// Serialization-safe reference to content whose exact version may be
/// unknown at the point of a user action.
///
/// This is a reference/selector, not a resolved identity. Use
/// [`ContentVersionId`] for exact resolver and installation records.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct ContentReference {
    /// Stable, version-independent content ID.
    pub id: String,
    /// Exact version when the caller already knows one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl<'de> Deserialize<'de> for ContentReference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct WireReference {
            id: String,
            #[serde(default)]
            version: Option<String>,
        }

        let wire = WireReference::deserialize(deserializer)?;
        Self::parse(wire.id, wire.version).map_err(serde::de::Error::custom)
    }
}

impl ContentReference {
    /// Validate and construct a reference at an API or persistence boundary.
    pub fn parse(id: impl Into<String>, version: Option<String>) -> Result<Self, String> {
        let id = id.into();
        id.parse::<VaporId>()
            .map_err(|error| format!("invalid Vapor content ID `{id}`: {error}"))?;
        if let Some(version) = version.as_deref() {
            Version::parse(version)
                .map_err(|error| format!("invalid exact content version `{version}`: {error}"))?;
        }
        Ok(Self { id, version })
    }

    /// Convert the version-independent part into the canonical typed ID.
    pub fn vapor_id(&self) -> Result<VaporId, ParseVaporIdError> {
        self.id.parse()
    }

    /// Convert to an exact version identity when a version is present.
    pub fn exact_version_id(&self) -> Result<Option<ContentVersionId>, String> {
        let Some(version) = self.version.as_deref() else {
            return Ok(None);
        };
        let id = self
            .vapor_id()
            .map_err(|error| format!("invalid Vapor content ID `{}`: {error}", self.id))?;
        let version = Version::parse(version)
            .map_err(|error| format!("invalid exact content version `{version}`: {error}"))?;
        Ok(Some(ContentVersionId { id, version }))
    }
}

impl From<ContentVersionId> for ContentReference {
    fn from(identity: ContentVersionId) -> Self {
        Self {
            id: identity.id.to_string(),
            version: Some(identity.version.to_string()),
        }
    }
}

impl From<VaporId> for ContentReference {
    fn from(id: VaporId) -> Self {
        Self {
            id: id.to_string(),
            version: None,
        }
    }
}

impl fmt::Display for ContentVersionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}@{}", self.id, self.version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vapor_id_is_version_independent() {
        let id: VaporId = "ghf-studios/example/physics".parse().unwrap();
        assert_eq!(id.as_str(), "ghf-studios/example/physics");
    }

    #[test]
    fn vapor_id_rejects_whitespace() {
        assert!("ghf studios/example".parse::<VaporId>().is_err());
    }

    #[test]
    fn content_version_id_formats_as_id_at_version() {
        let identity = ContentVersionId {
            id: "ghf-studios/example/physics".parse().unwrap(),
            version: Version::parse("2.3.0").unwrap(),
        };
        assert_eq!(identity.to_string(), "ghf-studios/example/physics@2.3.0");
    }
}
