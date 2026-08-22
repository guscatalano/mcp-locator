//! mcp-locator broker.
//!
//! This build serves the read-only half of the protocol (spec/002 §3): handshake, list, status.
//! Activation, consent, and the lifetime state machine are not present — a client that asks for
//! them gets `method not found` rather than a stub that looks like a grant.

use mcp_locator_broker::catalog::{enumerate, EnumerateOptions};
use mcp_locator_broker::dirs::{self, resolve_roots, Root, Tier};
use mcp_locator_broker::server;
use std::path::PathBuf;

const USAGE: &str = "mcp-locator-broker — local MCP server discovery

  mcp-locator-broker serve [--endpoint <pipe-or-socket>]
  mcp-locator-broker list [--all] [--roots <tier>=<path>[,<tier>=<path>...]]
  mcp-locator-broker dirs

`list` reads the same registry the served catalog does; --roots points it at a fixture tree,
which is how its output is cross-checked against the TypeScript client library.";

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("serve");

    match command {
        "serve" => {
            let endpoint = flag_value(&args, "--endpoint").unwrap_or_else(dirs::default_endpoint);
            server::listen(&endpoint).await
        }
        "list" => {
            let roots = match flag_value(&args, "--roots") {
                Some(spec) => parse_roots(&spec)?,
                None => resolve_roots(),
            };
            let lookup = |name: &str| std::env::var(name).ok();
            let catalog = enumerate(&EnumerateOptions {
                roots,
                lookup: &lookup,
                include_orphaned: args.iter().any(|a| a == "--all"),
            });
            println!("{}", serde_json::to_string_pretty(&catalog)?);
            Ok(())
        }
        "dirs" => {
            for root in resolve_roots() {
                let missing = if root.path.exists() {
                    ""
                } else {
                    "  (missing)"
                };
                println!("[{:?}] {}{}", root.tier, root.path.display(), missing);
            }
            println!("endpoint: {}", dirs::default_endpoint());
            Ok(())
        }
        "-h" | "--help" => {
            println!("{USAGE}");
            Ok(())
        }
        other => {
            eprintln!("unknown command: {other}\n\n{USAGE}");
            std::process::exit(2);
        }
    }
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let index = args.iter().position(|a| a == flag)?;
    args.get(index + 1).cloned()
}

/// Parse `system=/a,user=/b` into roots. Test and diagnostic affordance only — real deployments
/// use the platform directories.
fn parse_roots(spec: &str) -> std::io::Result<Vec<Root>> {
    spec.split(',')
        .filter(|s| !s.is_empty())
        .map(|pair| {
            let (tier, path) = pair.split_once('=').ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("expected <tier>=<path>, got `{pair}`"),
                )
            })?;
            let tier = Tier::parse(tier).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("unknown tier `{tier}`"),
                )
            })?;
            Ok(Root {
                tier,
                path: PathBuf::from(path),
            })
        })
        .collect()
}
