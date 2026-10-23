use crate::{ClientAction, ClientEvent, ClientRevision, ClientSnapshot, LegacyStorageSeed};
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 33;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientRequest {
    pub protocol_version: u16,
    pub request_id: RequestId,
    /// Product surface that initiated the request; used for operation logs.
    #[serde(default)]
    pub origin: ClientOrigin,
    pub expected_revision: Option<ClientRevision>,
    pub action: ClientAction,
}

/// Stable frontend identity attached to service requests and their output.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClientOrigin {
    Launcher,
    Studio,
    Ide,
    Installer,
    Cli,
    #[default]
    Unknown,
}

impl ClientOrigin {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Launcher => "launcher",
            Self::Studio => "studio",
            Self::Ide => "ide",
            Self::Installer => "installer",
            Self::Cli => "cli",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientResponse {
    pub protocol_version: u16,
    pub request_id: RequestId,
    pub revision: ClientRevision,
    pub kind: ClientResponseKind,
    /// Identity of the operation accepted for this request. This is distinct
    /// from the snapshot's primary operation, which may belong to unrelated work.
    #[serde(default)]
    pub operation_id: Option<String>,
    pub snapshot: ClientSnapshot,
    pub error: Option<ServiceError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClientResponseKind {
    Applied,
    OperationAccepted,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceError {
    pub code: ServiceErrorCode,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServiceErrorCode {
    UnsupportedProtocol,
    InvalidRequest,
    StaleRevision,
    ActionUnavailable,
    OperationInProgress,
    Conflict,
    ProviderFailure,
    PersistenceFailure,
    InternalFailure,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload", rename_all = "kebab-case")]
pub enum ProtocolFrame {
    Connect {
        protocol_version: u16,
        /// Present only when a frontend has legacy state to import. The
        /// service imports it only if no durable state exists yet.
        legacy_storage: Option<LegacyStorageSeed>,
    },
    Initialize {
        protocol_version: u16,
        legacy_storage: LegacyStorageSeed,
    },
    Request(ClientRequest),
    /// High-frequency LSP JSON-RPC traffic; it does not broadcast app snapshots.
    LanguageServerMessage(String),
    /// The frontend is closing its session and no longer needs service events.
    Disconnect,
    Response(Box<ClientResponse>),
    Event(ClientEvent),
}
