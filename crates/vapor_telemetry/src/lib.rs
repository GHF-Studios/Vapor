//! Local Vapor telemetry protocol, broker, producers, and consumers.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::env;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::{
    Arc, Mutex,
    mpsc::{self, Receiver, SyncSender},
};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const TELEMETRY_ADDR_ENV: &str = "VAPOR_TELEMETRY_ADDR";
pub const TELEMETRY_SESSION_ENV: &str = "VAPOR_TELEMETRY_SESSION";

const BROKER_QUEUE: usize = 4096;
const PRODUCER_QUEUE: usize = 256;
const CONSUMER_WRITE_TIMEOUT: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryEvent {
    pub timestamp_unix_ms: u64,
    pub source: String,
    pub payload: TelemetryPayload,
}

impl TelemetryEvent {
    pub fn new(source: impl Into<String>, payload: TelemetryPayload) -> Self {
        Self {
            timestamp_unix_ms: unix_time_ms(),
            source: source.into(),
            payload,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TelemetryPayload {
    Log {
        stream: String,
        message: String,
    },
    Metrics {
        values: BTreeMap<String, f64>,
    },
    Snapshot {
        topic: String,
        value: Value,
    },
    Lifecycle {
        state: String,
        detail: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ClientRole {
    Producer,
    Consumer,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct Hello {
    role: ClientRole,
}

#[derive(Clone)]
pub struct TelemetryPublisher {
    sender: SyncSender<TelemetryEvent>,
}

impl TelemetryPublisher {
    pub fn publish(&self, event: TelemetryEvent) -> bool {
        self.sender.try_send(event).is_ok()
    }

    pub fn payload(&self, source: impl Into<String>, payload: TelemetryPayload) -> bool {
        self.publish(TelemetryEvent::new(source, payload))
    }
}

pub struct TelemetryBroker {
    address: SocketAddr,
    publisher: TelemetryPublisher,
}

impl TelemetryBroker {
    pub fn start(event_log: impl AsRef<Path>) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let address = listener.local_addr()?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(event_log)?;
        let (sender, receiver) = mpsc::sync_channel(BROKER_QUEUE);
        let consumers = Arc::new(Mutex::new(Vec::<TcpStream>::new()));

        {
            let sender = sender.clone();
            let consumers = Arc::clone(&consumers);
            thread::spawn(move || accept_connections(listener, sender, consumers));
        }

        thread::spawn(move || dispatch_events(receiver, consumers, file));

        Ok(Self {
            address,
            publisher: TelemetryPublisher { sender },
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn publisher(&self) -> TelemetryPublisher {
        self.publisher.clone()
    }
}

fn accept_connections(
    listener: TcpListener,
    sender: SyncSender<TelemetryEvent>,
    consumers: Arc<Mutex<Vec<TcpStream>>>,
) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else {
            continue;
        };
        let sender = sender.clone();
        let consumers = Arc::clone(&consumers);
        thread::spawn(move || {
            let _ = handle_connection(stream, sender, consumers);
        });
    }
}

fn handle_connection(
    stream: TcpStream,
    sender: SyncSender<TelemetryEvent>,
    consumers: Arc<Mutex<Vec<TcpStream>>>,
) -> io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut hello = String::new();
    if reader.read_line(&mut hello)? == 0 {
        return Ok(());
    }
    let hello: Hello = serde_json::from_str(hello.trim_end()).map_err(invalid_data)?;

    match hello.role {
        ClientRole::Producer => {
            for line in reader.lines() {
                let Ok(line) = line else {
                    break;
                };
                let Ok(event) = serde_json::from_str::<TelemetryEvent>(&line) else {
                    continue;
                };
                let _ = sender.try_send(event);
            }
        }
        ClientRole::Consumer => {
            stream.set_nodelay(true)?;
            stream.set_write_timeout(Some(CONSUMER_WRITE_TIMEOUT))?;
            consumers
                .lock()
                .map_err(|_| io::Error::other("telemetry consumer lock poisoned"))?
                .push(stream);
        }
    }

    Ok(())
}

fn dispatch_events(
    receiver: Receiver<TelemetryEvent>,
    consumers: Arc<Mutex<Vec<TcpStream>>>,
    file: File,
) {
    let mut writer = BufWriter::new(file);

    for event in receiver {
        let Ok(mut encoded) = serde_json::to_vec(&event) else {
            continue;
        };
        encoded.push(b'\n');

        let _ = writer.write_all(&encoded);

        let Ok(mut consumers) = consumers.lock() else {
            continue;
        };
        consumers.retain_mut(|stream| stream.write_all(&encoded).is_ok());
    }
}

pub struct TelemetryEmitter {
    source: String,
    sender: SyncSender<TelemetryEvent>,
}

impl TelemetryEmitter {
    pub fn from_env(source: impl Into<String>) -> Option<Self> {
        let address = env::var(TELEMETRY_ADDR_ENV).ok()?;
        Self::connect(address, source).ok()
    }

    pub fn connect(
        address: impl ToSocketAddrs,
        source: impl Into<String>,
    ) -> io::Result<Self> {
        let mut stream = TcpStream::connect(address)?;
        stream.set_nodelay(true)?;
        write_json_line(
            &mut stream,
            &Hello {
                role: ClientRole::Producer,
            },
        )?;

        let (sender, receiver) = mpsc::sync_channel(PRODUCER_QUEUE);
        thread::spawn(move || {
            let mut writer = BufWriter::new(stream);
            for event in receiver {
                if write_json_line(&mut writer, &event).is_err() {
                    break;
                }
                if writer.flush().is_err() {
                    break;
                }
            }
        });

        Ok(Self {
            source: source.into(),
            sender,
        })
    }

    pub fn publish(&self, payload: TelemetryPayload) -> bool {
        self.sender
            .try_send(TelemetryEvent::new(self.source.clone(), payload))
            .is_ok()
    }

    pub fn publish_metrics(&self, values: BTreeMap<String, f64>) -> bool {
        self.publish(TelemetryPayload::Metrics { values })
    }

    pub fn publish_snapshot(&self, topic: impl Into<String>, value: Value) -> bool {
        self.publish(TelemetryPayload::Snapshot {
            topic: topic.into(),
            value,
        })
    }
}

pub struct TelemetryConsumer {
    reader: BufReader<TcpStream>,
}

impl TelemetryConsumer {
    pub fn connect(address: impl ToSocketAddrs) -> io::Result<Self> {
        let mut stream = TcpStream::connect(address)?;
        stream.set_nodelay(true)?;
        write_json_line(
            &mut stream,
            &Hello {
                role: ClientRole::Consumer,
            },
        )?;
        Ok(Self {
            reader: BufReader::new(stream),
        })
    }

    pub fn recv(&mut self) -> io::Result<Option<TelemetryEvent>> {
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        serde_json::from_str(line.trim_end())
            .map(Some)
            .map_err(invalid_data)
    }
}

pub fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn write_json_line(writer: &mut impl Write, value: &impl Serialize) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, value).map_err(invalid_data)?;
    writer.write_all(b"\n")
}

fn invalid_data(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}
