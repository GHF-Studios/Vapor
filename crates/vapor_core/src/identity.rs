//! Semantic identity for Vapor Content.

use semver::Version;
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use std::str::FromStr;

/// Immutable, human-readable semantic identity of one Vapor Content artifact.
///
/// A version is deliberately not part of the Vapor ID.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct VaporId(String);

impl VaporId {
    pub fn new(value: impl Into<String>) -> Result<Self, ParseVaporIdError> {
        let value = value.into();

        if value.is_empty() || value.chars().any(char::is_whitespace) {
            return Err(ParseVaporIdError);
        }

        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseVaporIdError;

impl fmt::Display for ParseVaporIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Vapor ID must be non-empty and contain no whitespace")
    }
}

impl std::error::Error for ParseVaporIdError {}

/// Exact identity of one version of Vapor Content.
///
/// This identity exists independently of dependency resolution. Multiple
/// versions of the same Vapor ID may coexist in one resolved composition.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ContentVersionId {
    pub id: VaporId,
    pub version: Version,
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
