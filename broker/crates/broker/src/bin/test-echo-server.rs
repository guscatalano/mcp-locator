//! Test fixture, not a product binary.
//!
//! A minimal stdio "MCP server": it reads newline-delimited JSON-RPC requests and answers each
//! one, so activation tests can prove a real client reached a real child process through the
//! broker's relay. It exits when its stdin closes, which is also what makes it a useful probe
//! for graceful shutdown.

use std::io::{BufRead, Write};

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }

        let id = serde_json::from_str::<serde_json::Value>(&line)
            .ok()
            .and_then(|v| v.get("id").cloned())
            .unwrap_or(serde_json::Value::Null);

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "echo": line, "pid": std::process::id() }
        });

        if writeln!(stdout, "{response}").is_err() || stdout.flush().is_err() {
            break;
        }
    }
}
