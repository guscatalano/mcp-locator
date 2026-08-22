//! mcp-locator broker.
//!
//! Serves discovery and activation over a local pipe: clients enumerate registered servers,
//! activate one (which starts it, refcounted, after a consent check), and release it. Consent
//! is currently granted through this CLI rather than an interactive prompt — the Win32 consent
//! dialog is the next piece. That makes `consent grant` a deliberate, auditable step rather
//! than something an AI client can trigger on its own.

use mcp_locator_broker::activation::Engine;
use mcp_locator_broker::audit::AuditLog;
use mcp_locator_broker::catalog::{enumerate, EnumerateOptions};
use mcp_locator_broker::consent::{ConsentScope, ConsentStore};
use mcp_locator_broker::dirs::{self, resolve_roots, resolve_state_dir, Root, Tier};
use mcp_locator_broker::server::{self, Broker};
use std::path::PathBuf;

const USAGE: &str = "mcp-locator-broker — local MCP server discovery and activation

  mcp-locator-broker serve [--endpoint <pipe-or-socket>]
  mcp-locator-broker list [--all] [--roots <tier>=<path>[,...]]
  mcp-locator-broker dirs
  mcp-locator-broker consent list
  mcp-locator-broker consent grant <name>
  mcp-locator-broker consent deny <name>
  mcp-locator-broker consent forget <name>

`consent grant` binds the approval to the card's current launch command. Editing that command
afterwards invalidates the approval, and the server will refuse to start until it is re-granted.";

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("serve");

    match command {
        "serve" => {
            let state_dir = resolve_state_dir();
            let engine = Engine::new(ConsentStore::load(&state_dir), AuditLog::new(&state_dir))?;
            let endpoint = flag_value(&args, "--endpoint").unwrap_or_else(dirs::default_endpoint);
            server::listen(Broker::new(engine), &endpoint).await
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
            println!("state:    {}", resolve_state_dir().display());
            println!("endpoint: {}", dirs::default_endpoint());
            Ok(())
        }

        "consent" => consent_command(&args),

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

fn consent_command(args: &[String]) -> std::io::Result<()> {
    let state_dir = resolve_state_dir();
    let mut store = ConsentStore::load(&state_dir);
    let action = args.get(1).map(String::as_str).unwrap_or("list");

    if action == "list" {
        if store.records().is_empty() {
            println!("No consent decisions recorded.");
        }
        for (name, record) in store.records() {
            println!(
                "{name}  {:?}  {}",
                record.state,
                record.granted_at.as_deref().unwrap_or("-")
            );
        }
        return Ok(());
    }

    let Some(name) = args.get(2) else {
        eprintln!("usage: mcp-locator-broker consent {action} <name>");
        std::process::exit(2);
    };

    match action {
        "grant" => {
            // Bind to the card as it exists right now: that hash is what makes the approval
            // specific to this launch command rather than to the name alone.
            let lookup = |var: &str| std::env::var(var).ok();
            let catalog = enumerate(&EnumerateOptions {
                roots: resolve_roots(),
                lookup: &lookup,
                include_orphaned: true,
            });
            let Some(entry) = catalog.entries.iter().find(|e| &e.name == name) else {
                eprintln!("unknown server: {name}");
                std::process::exit(1);
            };
            store.grant(name, &entry.launch_hash, ConsentScope::User)?;
            println!("granted {name}");
            println!("  launch: {}", launch_summary(entry));
            println!("  bound to {}", entry.launch_hash);
        }
        "deny" => {
            store.deny(name)?;
            println!("denied {name}");
        }
        "forget" => {
            store.forget(name)?;
            println!("forgot {name}");
        }
        other => {
            eprintln!("unknown consent action: {other}\n\n{USAGE}");
            std::process::exit(2);
        }
    }
    Ok(())
}

fn launch_summary(entry: &mcp_locator_broker::catalog::Entry) -> String {
    match entry.card.local.as_ref().and_then(|l| l.launch.as_ref()) {
        Some(launch) => {
            let args = launch
                .args
                .as_ref()
                .map(|a| a.join(" "))
                .unwrap_or_default();
            format!("{} {}", launch.command, args)
                .trim_end()
                .to_string()
        }
        None => "(endpoint only)".to_string(),
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
