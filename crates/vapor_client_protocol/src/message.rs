use crate::{ClientAction, ClientEvent, ClientRevision, ClientSnapshot, LegacyStorageSeed};
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 21;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientRequest {
    pub protocol_version: u16,
    pub request_id: RequestId,
    pub expected_revision: Option<ClientRevision>,
    pub action: ClientAction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientResponse {
    pub protocol_version: u16,
    pub request_id: RequestId,
    pub revision: ClientRevision,
    pub kind: ClientResponseKind,
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
    Response(Box<ClientResponse>),
    Event(ClientEvent),
}
