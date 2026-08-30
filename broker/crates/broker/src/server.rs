//! Pipe server and request dispatch (spec/002 §3).
//!
//! Framing is newline-delimited JSON-RPC. The transport differs per platform — Windows named
//! pipes, unix sockets elsewhere — but dispatch is shared, which is what keeps the later
//! platform ports additive.
//!
//! Grants are scoped to the connection that took them. That subsumes client-process death:
//! a client that crashes, exits, or closes cleanly all look identical from here, so its servers
//! are reclaimed without watching process handles.

use crate::activation::{ActivateError, Engine};
use crate::catalog::{enumerate, Catalog, Entry, EnumerateOptions};
use crate::consent::{ConsentScope, ConsentState};
use crate::dirs::{resolve_roots, resolve_state_dir};
use crate::prompt::{launch_summary, Decision, Prompter};
use mcp_locator_proto::rpc::{
    HandshakeResult, Request, Response, ServerState, BROKER_PROTOCOL, INVALID_PARAMS,
    INVALID_REQUEST, METHOD_NOT_FOUND, PARSE_ERROR,
};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

const BROKER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Broker error codes live below the JSON-RPC reserved range.
pub const CONSENT_REQUIRED: i32 = -32000;
pub const ACTIVATION_FAILED: i32 = -32001;
pub const IN_USE: i32 = -32002;

static NEXT_CONNECTION: AtomicU64 = AtomicU64::new(0);

pub struct Broker {
    pub engine: Mutex<Engine>,
    prompter: Prompter,
    /// Serializes consent dialogs. One prompt at a time is right on its own terms — stacked
    /// dialogs are how users get trained to click through — and it also collapses a race: two
    /// clients activating the same server produce one question, because whoever waits re-reads
    /// the store and finds the answer already there.
    prompt_lock: Mutex<()>,
}

impl Broker {
    pub fn new(engine: Engine) -> Arc<Self> {
        Arc::new(Self {
            engine: Mutex::new(engine),
            prompter: Prompter::discover(),
            prompt_lock: Mutex::new(()),
        })
    }

    /// Ask the user about `entry`, then record the answer. Returns the consent state as it
    /// stands afterwards. The engine lock is never held across the dialog: a human takes
    /// seconds at best, and the broker has to keep serving everyone else meanwhile.
    async fn seek_consent(&self, entry: &Entry, client_pid: Option<u32>) -> ConsentState {
        let _serialized = self.prompt_lock.lock().await;

        // Re-read after taking the lock: another connection may have just asked the same
        // question, and the user should not be shown it twice.
        let record = {
            let engine = self.engine.lock().await;
            engine.consent.evaluate(&entry.name, &entry.launch_hash)
        };
        if !Prompter::should_ask(record.state) {
            return record.state;
        }

        let decision = self.prompter.ask(entry, &record, client_pid).await;
        let mut engine = self.engine.lock().await;
        match decision {
            Decision::Allow => {
                let command = launch_summary(entry);
                if engine
                    .consent
                    .grant(
                        &entry.name,
                        &entry.launch_hash,
                        &command,
                        ConsentScope::User,
                    )
                    .is_err()
                {
                    // A grant that could not be written must not be honoured in memory only,
                    // or the answer silently evaporates on the next broker restart.
                    return ConsentState::NotAsked;
                }
                engine
                    .audit
                    .record("consent-granted", &entry.name, client_pid, "prompt");
                ConsentState::Granted
            }
            Decision::Deny => {
                let _ = engine.consent.deny(&entry.name);
                engine
                    .audit
                    .record("consent-denied", &entry.name, client_pid, "prompt");
                ConsentState::Denied
            }
            // No answer is not a decision: nothing is stored, so the next activation asks again.
            Decision::Unanswered => {
                engine
                    .audit
                    .record("consent-unanswered", &entry.name, client_pid, "prompt");
                record.state
            }
        }
    }
}

fn current_catalog(include_orphaned: bool) -> Catalog {
    let lookup = |name: &str| std::env::var(name).ok();
    enumerate(&EnumerateOptions {
        roots: resolve_roots(),
        lookup: &lookup,
        include_orphaned,
    })
}

fn find_entry(name: &str) -> Option<Entry> {
    current_catalog(true)
        .entries
        .into_iter()
        .find(|e| e.name == name)
}

/// Handle one request on behalf of one connection.
pub async fn dispatch(
    broker: &Arc<Broker>,
    request: &Request,
    connection: u64,
    client_pid: Option<u32>,
) -> Response {
    let id = request.id.clone();

    if request.jsonrpc != "2.0" {
        return Response::err(id, INVALID_REQUEST, "jsonrpc must be \"2.0\"");
    }

    let param_str = |key: &str| -> Option<String> {
        request
            .params
            .as_ref()
            .and_then(|p| p.get(key))
            .and_then(|v| v.as_str())
            .map(String::from)
    };
    let param_bool = |key: &str| -> bool {
        request
            .params
            .as_ref()
            .and_then(|p| p.get(key))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    };

    match request.method.as_str() {
        "locator/handshake" => Response::ok(
            id,
            serde_json::to_value(HandshakeResult {
                broker_version: BROKER_VERSION.to_string(),
                broker_protocol: BROKER_PROTOCOL,
            })
            .unwrap_or(Value::Null),
        ),

        "locator/list" => {
            let catalog = current_catalog(param_bool("includeOrphaned"));
            let mut engine = broker.engine.lock().await;

            let servers: Vec<Value> = catalog
                .entries
                .iter()
                .map(|entry| {
                    let consent = engine.consent.evaluate(&entry.name, &entry.launch_hash);
                    let grants = engine.grant_count(&entry.name);
                    let running = engine.is_running(&entry.name);
                    json!({
                        "name": entry.name,
                        "version": entry.version,
                        "title": entry.card.title,
                        "description": entry.card.description,
                        "tier": entry.tier,
                        "path": entry.path,
                        "orphaned": entry.orphaned,
                        "launchHash": entry.launch_hash,
                        "state": state_of(entry.orphaned, running, grants),
                        "consent": consent,
                        "grants": grants,
                        "shadowed": entry.shadowed,
                    })
                })
                .collect();

            Response::ok(
                id,
                json!({ "servers": servers, "diagnostics": catalog.diagnostics }),
            )
        }

        "locator/status" => {
            let Some(name) = param_str("name") else {
                return Response::err(id, INVALID_PARAMS, "`name` is required");
            };
            let Some(entry) = find_entry(&name) else {
                return Response::err(id, INVALID_PARAMS, format!("unknown server: {name}"));
            };

            let mut engine = broker.engine.lock().await;
            let grants = engine.grant_count(&name);
            let running = engine.is_running(&name);
            let consent = engine.consent.evaluate(&name, &entry.launch_hash);

            Response::ok(
                id,
                json!({
                    "name": name,
                    "state": state_of(entry.orphaned, running, grants),
                    "grants": grants,
                    "holders": engine.holders(&name),
                    "consent": consent,
                }),
            )
        }

        "locator/consent/query" => {
            let Some(name) = param_str("name") else {
                return Response::err(id, INVALID_PARAMS, "`name` is required");
            };
            let Some(entry) = find_entry(&name) else {
                return Response::err(id, INVALID_PARAMS, format!("unknown server: {name}"));
            };
            let engine = broker.engine.lock().await;
            Response::ok(
                id,
                json!(engine.consent.evaluate(&name, &entry.launch_hash)),
            )
        }

        "locator/activate" => {
            let Some(name) = param_str("name") else {
                return Response::err(id, INVALID_PARAMS, "`name` is required");
            };
            let Some(entry) = find_entry(&name) else {
                return Response::err(id, INVALID_PARAMS, format!("unknown server: {name}"));
            };

            // Consent first, outside the engine lock, so a dialog waiting on a human does not
            // stall every other client. `activate` re-checks it regardless — this only decides
            // whether to ask, never whether to allow.
            let state = {
                let engine = broker.engine.lock().await;
                engine
                    .consent
                    .evaluate(&entry.name, &entry.launch_hash)
                    .state
            };
            if Prompter::should_ask(state) && broker.prompter.available() {
                broker.seek_consent(&entry, client_pid).await;
            }

            let mut engine = broker.engine.lock().await;
            match engine.activate(&entry, connection, client_pid).await {
                Ok(result) => Response::ok(id, json!(result)),
                // Consent gets its own code so clients can route it to a "ask the user" path
                // rather than surfacing it as a generic failure.
                Err(e @ ActivateError::ConsentRequired { .. }) => {
                    Response::err(id, CONSENT_REQUIRED, e.to_string())
                }
                Err(e) => Response::err(id, ACTIVATION_FAILED, e.to_string()),
            }
        }

        "locator/release" => {
            let Some(grant_id) = param_str("grantId") else {
                return Response::err(id, INVALID_PARAMS, "`grantId` is required");
            };
            let released = broker.engine.lock().await.release(&grant_id).await;
            if released {
                Response::ok(id, json!({ "released": grant_id }))
            } else {
                Response::err(id, INVALID_PARAMS, format!("unknown grant: {grant_id}"))
            }
        }

        "locator/deactivate" => {
            let Some(name) = param_str("name") else {
                return Response::err(id, INVALID_PARAMS, "`name` is required");
            };
            let force = param_bool("force");
            let mut engine = broker.engine.lock().await;
            match engine.deactivate(&name, force).await {
                Ok(count) => Response::ok(id, json!({ "name": name, "grantsReleased": count })),
                // Naming the holders is what lets a user decide whether forcing is safe.
                Err(holders) => Response::err(
                    id,
                    IN_USE,
                    format!(
                        "{name} is in use by {}; pass force to stop it anyway",
                        holders.join(", ")
                    ),
                ),
            }
        }

        other => Response::err(id, METHOD_NOT_FOUND, format!("unknown method: {other}")),
    }
}

fn state_of(orphaned: bool, running: bool, grants: usize) -> ServerState {
    match (orphaned, running, grants) {
        (true, _, _) => ServerState::Orphaned,
        (_, true, 0) => ServerState::Idle,
        (_, true, _) => ServerState::Running,
        _ => ServerState::Registered,
    }
}

/// Serve newline-delimited JSON-RPC over one connection, releasing its grants when it ends.
pub async fn serve_connection<S>(
    broker: Arc<Broker>,
    stream: S,
    client_pid: Option<u32>,
) -> std::io::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let connection = NEXT_CONNECTION.fetch_add(1, Ordering::Relaxed);
    let (read_half, mut write_half) = tokio::io::split(stream);
    let mut lines = BufReader::new(read_half).lines();

    let result = async {
        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }

            let response = match serde_json::from_str::<Request>(&line) {
                Ok(request) => dispatch(&broker, &request, connection, client_pid).await,
                // Without a parsed id, JSON-RPC says to answer with a null id.
                Err(e) => Response::err(Value::Null, PARSE_ERROR, e.to_string()),
            };

            let mut encoded = serde_json::to_string(&response).unwrap_or_else(|e| {
                format!(
                    r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":-32603,"message":"{e}"}}}}"#
                )
            });
            encoded.push('\n');
            write_half.write_all(encoded.as_bytes()).await?;
            write_half.flush().await?;
        }
        Ok::<(), std::io::Error>(())
    }
    .await;

    // Runs whether the loop ended cleanly or the connection broke: a crashed client must not
    // leave its servers running.
    broker
        .engine
        .lock()
        .await
        .release_connection(connection)
        .await;
    result
}

/// Stop servers whose idle window has elapsed. Driven on a tick rather than from a timer
/// callback so every lifetime decision happens under the same lock.
pub fn spawn_idle_reaper(broker: Arc<Broker>) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            ticker.tick().await;
            broker.engine.lock().await.reap_idle().await;
        }
    });
}

pub fn state_dir() -> std::path::PathBuf {
    resolve_state_dir()
}

#[cfg(windows)]
pub async fn listen(broker: Arc<Broker>, endpoint: &str) -> std::io::Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;

    // NOTE (spec/003 §5): default pipe security descriptor for now. Explicit SDDL — user SID
    // plus a connect-only ACE for low integrity — lands with the hardening step.
    eprintln!("mcp-locator broker listening on {endpoint}");
    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(endpoint)?;
    spawn_idle_reaper(Arc::clone(&broker));

    loop {
        server.connect().await?;
        let connected = server;
        // Create the next instance before handling this one, or a client connecting during
        // request handling would get ERROR_PIPE_BUSY.
        server = ServerOptions::new().create(endpoint)?;

        let client_pid = crate::activation::peer_pid(&connected);
        let broker = Arc::clone(&broker);
        tokio::spawn(async move {
            if let Err(e) = serve_connection(broker, connected, client_pid).await {
                eprintln!("connection error: {e}");
            }
        });
    }
}

#[cfg(unix)]
pub async fn listen(broker: Arc<Broker>, endpoint: &str) -> std::io::Result<()> {
    use tokio::net::UnixListener;

    // A socket file from a previous run blocks bind; the singleton mutex is what actually
    // prevents two live brokers (spec/002 §2), so a stale file here is safe to clear.
    let _ = std::fs::remove_file(endpoint);
    let listener = UnixListener::bind(endpoint)?;
    eprintln!("mcp-locator broker listening on {endpoint}");
    spawn_idle_reaper(Arc::clone(&broker));

    loop {
        let (stream, _) = listener.accept().await?;
        let client_pid = crate::activation::peer_pid(&stream);
        let broker = Arc::clone(&broker);
        tokio::spawn(async move {
            if let Err(e) = serve_connection(broker, stream, client_pid).await {
                eprintln!("connection error: {e}");
            }
        });
    }
}
