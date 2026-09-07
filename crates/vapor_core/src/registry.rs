use reqwest::Url;
use reqwest::blocking::Client;
use serde::Deserialize;
use std::fmt;

const SUPPORTED_REGISTRY_SCHEMA: u32 = 1;

#[derive(Debug, Clone)]
pub struct RegistryClient {
    endpoint: Url,
    http: Client,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegisteredEcosystem {
    pub id: String,
    pub namespace: String,
    pub name: String,
    pub display_name: String,
    pub repositories: Vec<RegisteredRepository>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegisteredRepository {
    pub id: String,
    pub provider: String,
    pub owner: String,
    pub name: String,
    pub kind: String,
    pub checkout_path: String,
    pub web_url: String,
    pub clone_url: String,
}

#[derive(Debug, Deserialize)]
struct EcosystemResponse {
    schema_version: u32,
    ecosystem: EcosystemRecord,
    repositories: Vec<RegisteredRepository>,
}

#[derive(Debug, Deserialize)]
struct EcosystemRecord {
    id: String,
    namespace: String,
    name: String,
    display_name: String,
}

impl RegistryClient {
    pub fn new(endpoint: &str) -> Result<Self, RegistryError> {
        let endpoint = Url::parse(endpoint).map_err(|error| RegistryError::InvalidEndpoint {
            endpoint: endpoint.to_owned(),
            message: error.to_string(),
        })?;

        if !matches!(endpoint.scheme(), "http" | "https") || endpoint.host_str().is_none() {
            return Err(RegistryError::InvalidEndpoint {
                endpoint: endpoint.to_string(),
                message: "expected an absolute HTTP(S) URL".to_owned(),
            });
        }

        let http = Client::builder()
            .user_agent(format!("Vapor/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| RegistryError::Client(error.to_string()))?;

        Ok(Self { endpoint, http })
    }

    pub fn endpoint(&self) -> &str {
        self.endpoint.as_str()
    }

    pub fn ecosystem(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<RegisteredEcosystem, RegistryError> {
        let mut url = self.endpoint.clone();

        let base_path = url.path().trim_end_matches('/');

        url.set_path(&format!("{base_path}/v1/ecosystems/{namespace}/{name}"));

        let request_url = url.to_string();

        let response = self
            .http
            .get(url)
            .send()
            .map_err(|error| RegistryError::Request {
                url: request_url.clone(),
                message: error.to_string(),
            })?;

        let status = response.status();

        let body = response.text().map_err(|error| RegistryError::Request {
            url: request_url.clone(),
            message: error.to_string(),
        })?;

        if !status.is_success() {
            return Err(RegistryError::Http {
                url: request_url,
                status: status.as_u16(),
                body,
            });
        }

        let response: EcosystemResponse =
            serde_json::from_str(&body).map_err(|error| RegistryError::InvalidResponse {
                url: request_url,
                message: error.to_string(),
            })?;

        if response.schema_version != SUPPORTED_REGISTRY_SCHEMA {
            return Err(RegistryError::UnsupportedSchema {
                found: response.schema_version,
                supported: SUPPORTED_REGISTRY_SCHEMA,
            });
        }

        let returned_id = response.ecosystem.id.clone();
        let requested_id = format!("{namespace}/{name}");

        if returned_id != requested_id {
            return Err(RegistryError::IdentityMismatch {
                requested: requested_id,
                returned: returned_id,
            });
        }

        Ok(RegisteredEcosystem {
            id: response.ecosystem.id,
            namespace: response.ecosystem.namespace,
            name: response.ecosystem.name,
            display_name: response.ecosystem.display_name,
            repositories: response.repositories,
        })
    }
}

#[derive(Debug)]
pub enum RegistryError {
    InvalidEndpoint {
        endpoint: String,
        message: String,
    },

    Client(String),

    Request {
        url: String,
        message: String,
    },

    Http {
        url: String,
        status: u16,
        body: String,
    },

    InvalidResponse {
        url: String,
        message: String,
    },

    UnsupportedSchema {
        found: u32,
        supported: u32,
    },

    IdentityMismatch {
        requested: String,
        returned: String,
    },
}

impl fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEndpoint { endpoint, message } => {
                write!(
                    formatter,
                    "invalid Registry endpoint `{endpoint}`: {message}"
                )
            }

            Self::Client(message) => {
                write!(formatter, "failed to create Registry client: {message}")
            }

            Self::Request { url, message } => {
                write!(formatter, "Registry request `{url}` failed: {message}")
            }

            Self::Http { url, status, body } => {
                write!(
                    formatter,
                    "Registry request `{url}` returned HTTP {status}: {}",
                    body.trim()
                )
            }

            Self::InvalidResponse { url, message } => {
                write!(
                    formatter,
                    "invalid Registry response from `{url}`: {message}"
                )
            }

            Self::UnsupportedSchema { found, supported } => {
                write!(
                    formatter,
                    "unsupported Registry schema {found}; Vapor supports {supported}"
                )
            }

            Self::IdentityMismatch {
                requested,
                returned,
            } => {
                write!(
                    formatter,
                    "Registry returned ecosystem `{returned}` while resolving `{requested}`"
                )
            }
        }
    }
}

impl std::error::Error for RegistryError {}
