//! Pipe server and request dispatch (spec/002 §3).
//!
//! Framing is newline-delimited JSON-RPC. The transport differs per platform — Windows named
//! pipes, unix sockets elsewhere — but dispatch is shared, which is what keeps the later
//! platform ports additive.

use crate::catalog::{enumerate, Catalog, EnumerateOptions};
use crate::dirs::resolve_roots;
use mcp_locator_proto::rpc::{
    HandshakeResult, Request, Response, ServerState, BROKER_PROTOCOL, INVALID_PARAMS,
    INVALID_REQUEST, METHOD_NOT_FOUND, PARSE_ERROR,
};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const BROKER_VERSION: &str = env!("CARGO_PKG_VERSION");

fn current_catalog(include_orphaned: bool) -> Catalog {
    let lookup = |name: &str| std::env::var(name).ok();
    enumerate(&EnumerateOptions {
        roots: resolve_roots(),
        lookup: &lookup,
        include_orphaned,
    })
}

/// Handle one request. Read-only in this slice: activation, consent, and lifetime methods are
/// deliberately absent rather than stubbed, so a client cannot mistake a stub for a grant.
pub fn dispatch(request: &Request) -> Response {
    let id = request.id.clone();

    if request.jsonrpc != "2.0" {
        return Response::err(id, INVALID_REQUEST, "jsonrpc must be \"2.0\"");
    }

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
            let include_orphaned = request
                .params
                .as_ref()
                .and_then(|p| p.get("includeOrphaned"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let catalog = current_catalog(include_orphaned);
            let servers: Vec<Value> = catalog
                .entries
                .iter()
                .map(|entry| {
                    json!({
                        "name": entry.name,
                        "version": entry.version,
                        "title": entry.card.title,
                        "description": entry.card.description,
                        "tier": entry.tier,
                        "path": entry.path,
                        "orphaned": entry.orphaned,
                        "launchHash": entry.launch_hash,
                        "state": state_of(entry.orphaned),
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
            let Some(name) = request
                .params
                .as_ref()
                .and_then(|p| p.get("name"))
                .and_then(|v| v.as_str())
            else {
                return Response::err(id, INVALID_PARAMS, "`name` is required");
            };

            let catalog = current_catalog(true);
            match catalog.entries.iter().find(|e| e.name == name) {
                None => Response::err(id, INVALID_PARAMS, format!("unknown server: {name}")),
                Some(entry) => Response::ok(
                    id,
                    json!({
                        "name": entry.name,
                        "state": state_of(entry.orphaned),
                        // No activation engine yet, so there are no grants to report and no
                        // process to have started. Reported honestly rather than faked.
                        "grants": 0,
                        "pid": Value::Null,
                        "since": Value::Null,
                    }),
                ),
            }
        }

        other => Response::err(id, METHOD_NOT_FOUND, format!("unknown method: {other}")),
    }
}

fn state_of(orphaned: bool) -> ServerState {
    if orphaned {
        ServerState::Orphaned
    } else {
        ServerState::Registered
    }
}

/// Serve newline-delimited JSON-RPC over one connection until the peer disconnects.
pub async fn serve_connection<S>(stream: S) -> std::io::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (read_half, mut write_half) = tokio::io::split(stream);
    let mut lines = BufReader::new(read_half).lines();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => dispatch(&request),
            // Without a parsed id, JSON-RPC says to answer with a null id.
            Err(e) => Response::err(Value::Null, PARSE_ERROR, e.to_string()),
        };

        let mut encoded = serde_json::to_string(&response).unwrap_or_else(|e| {
            format!(r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":-32603,"message":"{e}"}}}}"#)
        });
        encoded.push('\n');
        write_half.write_all(encoded.as_bytes()).await?;
        write_half.flush().await?;
    }

    Ok(())
}

#[cfg(windows)]
pub async fn listen(endpoint: &str) -> std::io::Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;

    // NOTE (spec/003 §5): this uses the default pipe security descriptor. Explicit SDDL — user
    // SID plus a connect-only ACE for low integrity — lands with the hardening step. Everything
    // served here is already world-readable card data, so the gap is bounded to that.
    eprintln!("mcp-locator broker listening on {endpoint}");
    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(endpoint)?;

    loop {
        server.connect().await?;
        let connected = server;
        // Create the next instance before handling this one, or a client connecting during
        // request handling would get ERROR_PIPE_BUSY.
        server = ServerOptions::new().create(endpoint)?;

        tokio::spawn(async move {
            if let Err(e) = serve_connection(connected).await {
                eprintln!("connection error: {e}");
            }
        });
    }
}

#[cfg(unix)]
pub async fn listen(endpoint: &str) -> std::io::Result<()> {
    use tokio::net::UnixListener;

    // A socket file from a previous run blocks bind; the singleton mutex is what actually
    // prevents two live brokers (spec/002 §2), so a stale file here is safe to clear.
    let _ = std::fs::remove_file(endpoint);
    let listener = UnixListener::bind(endpoint)?;
    eprintln!("mcp-locator broker listening on {endpoint}");

    loop {
        let (stream, _) = listener.accept().await?;
        tokio::spawn(async move {
            if let Err(e) = serve_connection(stream).await {
                eprintln!("connection error: {e}");
            }
        });
    }
}
