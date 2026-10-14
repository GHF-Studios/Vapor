//! Structured operation host for GUI clients such as the in-game IDE.
//!
//! The host is intentionally a short-lived process boundary. Clients send one
//! JSON request per line and receive one JSON response per line. Requests call
//! the same Rust discovery and resolution APIs used by Vapor itself; this is
//! transport, not a second content implementation.

use crate::{VaporId, discover_local_content, resolve_local_content};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

pub const IDE_HOST_PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    protocol: u32,
    request_id: String,
    operation: String,
    #[serde(default)]
    arguments: Value,
}

#[derive(Debug, Serialize)]
struct Response {
    protocol: u32,
    request_id: String,
    result: Option<Value>,
    error: Option<String>,
}

/// Serve newline-delimited operations on stdin/stdout until EOF.
///
/// Diagnostics are returned as protocol errors. The process writes no human
/// output to stdout, so callers can parse every line as a response.
pub fn run_stdio() -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::BufWriter::new(io::stdout().lock());

    for line in stdin.lock().lines() {
        let line = line?;
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => {
                let request_id = request.request_id.clone();
                let outcome = dispatch(request);
                match outcome {
                    Ok(result) => Response {
                        protocol: IDE_HOST_PROTOCOL_VERSION,
                        request_id,
                        result: Some(result),
                        error: None,
                    },
                    Err(error) => Response {
                        protocol: IDE_HOST_PROTOCOL_VERSION,
                        request_id,
                        result: None,
                        error: Some(error),
                    },
                }
            }
            Err(error) => Response {
                protocol: IDE_HOST_PROTOCOL_VERSION,
                request_id: String::new(),
                result: None,
                error: Some(format!("invalid request: {error}")),
            },
        };

        serde_json::to_writer(&mut stdout, &response)?;
        stdout.write_all(b"\n")?;
        stdout.flush()?;
    }

    Ok(())
}

fn dispatch(request: Request) -> Result<Value, String> {
    if request.protocol != IDE_HOST_PROTOCOL_VERSION {
        return Err(format!(
            "protocol version {} is unsupported; this host supports {}",
            request.protocol, IDE_HOST_PROTOCOL_VERSION
        ));
    }

    match request.operation.as_str() {
        "content.discover" => discover(request.arguments),
        "content.resolve" => resolve(request.arguments),
        operation => Err(format!("unknown operation `{operation}`")),
    }
}

fn source_root(arguments: &Value) -> Result<PathBuf, String> {
    let path = arguments
        .get("source_root")
        .and_then(Value::as_str)
        .ok_or_else(|| "`source_root` must be a path string".to_owned())?;
    let root = PathBuf::from(path);
    if !root.is_absolute() {
        return Err("`source_root` must be absolute".to_owned());
    }
    Ok(root)
}

fn discover(arguments: Value) -> Result<Value, String> {
    let root = source_root(&arguments)?;
    let catalog = discover_local_content(&root).map_err(|error| error.to_string())?;
    let contents = catalog
        .iter()
        .map(|content| {
            json!({
                "id": content.manifest.content.id.as_str(),
                "version": content.manifest.content.version.to_string(),
                "kind": content.manifest.content.kind.as_str(),
                "root": content.root.display().to_string(),
                "manifest_path": content.manifest_path.display().to_string(),
                "manifest": &content.manifest,
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({ "source_root": root, "contents": contents }))
}

fn resolve(arguments: Value) -> Result<Value, String> {
    let root = source_root(&arguments)?;
    let id = arguments
        .get("content_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "`content_id` must be a Vapor ID string".to_owned())?
        .parse::<VaporId>()
        .map_err(|error| error.to_string())?;
    let catalog = discover_local_content(&root).map_err(|error| error.to_string())?;
    let graph = resolve_local_content(&catalog, &id).map_err(|error| error.to_string())?;
    let nodes = graph
        .nodes
        .values()
        .map(|node| {
            json!({
                "identity": node.identity.to_string(),
                "kind": node.kind.as_str(),
                "dependencies": node.dependencies.iter().map(|(alias, identity)| (alias.clone(), identity.to_string())).collect::<std::collections::BTreeMap<_, _>>(),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({ "root": graph.root, "nodes": nodes }))
}
