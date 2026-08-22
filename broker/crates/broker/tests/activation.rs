//! Activation, refcounting, and lifetime (spec/002 §4).
//!
//! These drive the engine directly rather than through the pipe, so the assertions are about
//! lifetime semantics rather than transport. Each test builds its own registry and state
//! directory in a temp dir — nothing here touches the user's real registry or consent store.

use mcp_locator_broker::activation::{ActivateError, Engine};
use mcp_locator_broker::audit::AuditLog;
use mcp_locator_broker::catalog::{enumerate, Entry, EnumerateOptions};
use mcp_locator_broker::consent::{ConsentScope, ConsentStore};
use mcp_locator_broker::dirs::{Root, Tier};
use std::path::PathBuf;

/// A stdio server fixture that echoes newline-delimited JSON-RPC back to its caller.
const ECHO_SERVER: &str = env!("CARGO_BIN_EXE_test-echo-server");

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("mcp-locator-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("servers")).unwrap();
        std::fs::create_dir_all(dir.join("state")).unwrap();
        Self { dir }
    }

    fn write_card(&self, name: &str, body: serde_json::Value) {
        std::fs::write(
            self.dir.join("servers").join(format!("{name}.card.json")),
            serde_json::to_string_pretty(&body).unwrap(),
        )
        .unwrap();
    }

    fn stdio_card(&self, name: &str) {
        self.write_card(
            name,
            serde_json::json!({
                "name": name,
                "version": "1.0.0",
                "description": "echo fixture",
                "local": { "launch": { "type": "stdio", "command": ECHO_SERVER } }
            }),
        );
    }

    fn catalog(&self) -> Vec<Entry> {
        let lookup = |var: &str| std::env::var(var).ok();
        enumerate(&EnumerateOptions {
            roots: vec![Root {
                tier: Tier::User,
                path: self.dir.join("servers"),
            }],
            lookup: &lookup,
            include_orphaned: true,
        })
        .entries
    }

    fn entry(&self, name: &str) -> Entry {
        self.catalog()
            .into_iter()
            .find(|e| e.name == name)
            .expect("card must be registered")
    }

    fn state_dir(&self) -> PathBuf {
        self.dir.join("state")
    }

    fn engine(&self) -> Engine {
        Engine::new(ConsentStore::load(&self.state_dir()), AuditLog::disabled()).unwrap()
    }

    fn granted_engine(&self, name: &str) -> Engine {
        let mut store = ConsentStore::load(&self.state_dir());
        store
            .grant(name, &self.entry(name).launch_hash, ConsentScope::User)
            .unwrap();
        Engine::new(store, AuditLog::disabled()).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[tokio::test]
async fn activation_without_consent_is_refused() {
    let fixture = Fixture::new("no-consent");
    fixture.stdio_card("com.example.echo");
    let mut engine = fixture.engine();

    let result = engine
        .activate(&fixture.entry("com.example.echo"), 1, Some(42))
        .await;

    match result {
        Err(ActivateError::ConsentRequired { name, .. }) => assert_eq!(name, "com.example.echo"),
        other => panic!("expected consent to be required, got {other:?}"),
    }
}

#[tokio::test]
async fn editing_the_launch_command_invalidates_consent() {
    let fixture = Fixture::new("stale-consent");
    fixture.stdio_card("com.example.echo");
    let mut engine = fixture.granted_engine("com.example.echo");

    // Swap the launch command behind the approval — the exact attack the hash binding exists
    // to stop. The card is otherwise identical, including its name and version.
    fixture.write_card(
        "com.example.echo",
        serde_json::json!({
            "name": "com.example.echo",
            "version": "1.0.0",
            "description": "echo fixture",
            "local": { "launch": { "type": "stdio", "command": ECHO_SERVER, "args": ["--now-different"] } }
        }),
    );

    let result = engine
        .activate(&fixture.entry("com.example.echo"), 1, Some(42))
        .await;
    assert!(
        matches!(result, Err(ActivateError::ConsentRequired { .. })),
        "a card edited after approval must not launch"
    );
}

#[tokio::test]
async fn activate_starts_a_server_and_release_stops_it() {
    let fixture = Fixture::new("lifecycle");
    fixture.stdio_card("com.example.echo");
    let mut engine = fixture.granted_engine("com.example.echo");
    let entry = fixture.entry("com.example.echo");

    let grant = engine
        .activate(&entry, 1, Some(42))
        .await
        .expect("activation must succeed");
    assert_eq!(engine.grant_count("com.example.echo"), 1);
    assert!(engine.is_running("com.example.echo"));

    assert!(engine.release(&grant.grant_id).await);
    assert_eq!(engine.grant_count("com.example.echo"), 0);
    assert!(
        !engine.is_running("com.example.echo"),
        "last release must stop the server"
    );
}

#[tokio::test]
async fn two_clients_hold_independent_grants() {
    let fixture = Fixture::new("two-clients");
    fixture.stdio_card("com.example.echo");
    let mut engine = fixture.granted_engine("com.example.echo");
    let entry = fixture.entry("com.example.echo");

    let a = engine.activate(&entry, 1, Some(11)).await.unwrap();
    let b = engine.activate(&entry, 2, Some(22)).await.unwrap();
    assert_eq!(engine.grant_count("com.example.echo"), 2);
    assert_ne!(a.grant_id, b.grant_id);
    assert_ne!(
        a.connection.address, b.connection.address,
        "each grant gets its own relay"
    );

    // One client going away must not disturb the other.
    engine.release(&a.grant_id).await;
    assert_eq!(engine.grant_count("com.example.echo"), 1);
    assert!(engine.is_running("com.example.echo"));

    engine.release(&b.grant_id).await;
    assert!(!engine.is_running("com.example.echo"));
}

#[tokio::test]
async fn a_dead_connection_releases_the_grants_it_held() {
    let fixture = Fixture::new("dead-connection");
    fixture.stdio_card("com.example.echo");
    let mut engine = fixture.granted_engine("com.example.echo");
    let entry = fixture.entry("com.example.echo");

    engine.activate(&entry, 7, Some(11)).await.unwrap();
    engine.activate(&entry, 7, Some(11)).await.unwrap();
    engine.activate(&entry, 8, Some(22)).await.unwrap();
    assert_eq!(engine.grant_count("com.example.echo"), 3);

    // Connection 7 dies (crash, exit, or close — indistinguishable from here).
    engine.release_connection(7).await;
    assert_eq!(
        engine.grant_count("com.example.echo"),
        1,
        "only the dead client's grants go"
    );
    assert!(engine.is_running("com.example.echo"));

    engine.release_connection(8).await;
    assert!(!engine.is_running("com.example.echo"));
}

#[tokio::test]
async fn deactivate_refuses_while_in_use_and_names_the_holders() {
    let fixture = Fixture::new("deactivate");
    fixture.stdio_card("com.example.echo");
    let mut engine = fixture.granted_engine("com.example.echo");
    let entry = fixture.entry("com.example.echo");

    engine.activate(&entry, 1, Some(4242)).await.unwrap();

    match engine.deactivate("com.example.echo", false).await {
        Err(holders) => assert!(
            holders.iter().any(|h| h.contains("4242")),
            "refusal must say who is holding it, got {holders:?}"
        ),
        Ok(_) => panic!("deactivate must refuse while a grant is held"),
    }

    let released = engine
        .deactivate("com.example.echo", true)
        .await
        .expect("force must succeed");
    assert_eq!(released, 1);
    assert!(!engine.is_running("com.example.echo"));
}

#[tokio::test]
async fn an_orphaned_card_never_launches() {
    let fixture = Fixture::new("orphan");
    fixture.write_card(
        "com.example.gone",
        serde_json::json!({
            "name": "com.example.gone",
            "version": "1.0.0",
            "description": "launch command does not exist",
            "local": { "launch": { "type": "stdio", "command": "/definitely/not/here/server.exe" } }
        }),
    );
    let mut engine = fixture.granted_engine("com.example.gone");

    let result = engine
        .activate(&fixture.entry("com.example.gone"), 1, None)
        .await;
    assert!(matches!(result, Err(ActivateError::Orphaned(_))));
}

#[tokio::test]
async fn a_client_reaches_the_real_server_through_the_relay() {
    let fixture = Fixture::new("relay");
    fixture.stdio_card("com.example.echo");
    let mut engine = fixture.granted_engine("com.example.echo");
    let entry = fixture.entry("com.example.echo");

    let grant = engine.activate(&entry, 1, Some(42)).await.unwrap();
    let response = round_trip(&grant.connection.address).await;

    assert_eq!(
        response["id"], 1,
        "the child must answer the request we sent"
    );
    assert!(
        response["result"]["echo"]
            .as_str()
            .unwrap()
            .contains("ping"),
        "payload must survive the relay verbatim: {response}"
    );

    engine.release(&grant.grant_id).await;
}

/// Connect to a grant's relay address, send one request, read one response.
async fn round_trip(address: &str) -> serde_json::Value {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let stream = open_relay(address).await;
    let (read_half, mut write_half) = tokio::io::split(stream);
    write_half
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n")
        .await
        .unwrap();
    write_half.flush().await.unwrap();

    let mut line = String::new();
    BufReader::new(read_half)
        .read_line(&mut line)
        .await
        .unwrap();
    serde_json::from_str(&line).expect("server must answer with JSON")
}

#[cfg(windows)]
async fn open_relay(address: &str) -> tokio::net::windows::named_pipe::NamedPipeClient {
    use tokio::net::windows::named_pipe::ClientOptions;

    // The broker accepts in the background, so the pipe may not be listening for an instant.
    for _ in 0..50 {
        match ClientOptions::new().open(address) {
            Ok(client) => return client,
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
    panic!("relay pipe never became available: {address}");
}

#[cfg(unix)]
async fn open_relay(address: &str) -> tokio::net::UnixStream {
    for _ in 0..50 {
        match tokio::net::UnixStream::connect(address).await {
            Ok(stream) => return stream,
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
    panic!("relay socket never became available: {address}");
}
