//! The derived catalog (spec/002 §1).
//!
//! The broker owns no registration data: this is rebuilt from the card files on disk, so the
//! broker can be killed, reinstalled, or superseded without losing anything. Behaviour must
//! match `catalog.ts` — `conformance/expected/basic.json` is the shared proof.

use crate::dirs::{Root, Tier};
use mcp_locator_proto::card::DiagnosticCode;
use mcp_locator_proto::{expand_card, launch_hash, validate, Card};
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub const CARD_SUFFIX: &str = ".card.json";

#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub path: PathBuf,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShadowedBy {
    pub path: PathBuf,
    pub tier: Tier,
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub name: String,
    #[serde(skip_serializing)]
    pub card: Card,
    /// Expanded card as raw JSON — hashing operates on these bytes so the digest matches the
    /// TypeScript implementation exactly. Retained for the activation engine, which needs the
    /// full launch stanza (env included) that the typed `Card` does not carry.
    #[serde(skip_serializing)]
    #[allow(dead_code)]
    pub expanded: Value,
    pub version: String,
    pub tier: Tier,
    pub path: PathBuf,
    pub orphaned: bool,
    pub shadowed: Vec<ShadowedBy>,
    #[serde(rename = "launchHash")]
    pub launch_hash: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Catalog {
    pub entries: Vec<Entry>,
    pub diagnostics: Vec<Diagnostic>,
}

pub struct EnumerateOptions<'a> {
    pub roots: Vec<Root>,
    pub lookup: &'a dyn Fn(&str) -> Option<String>,
    pub include_orphaned: bool,
}

/// Merge every registry tier into one catalog. Pure disk reads — no launching, no side effects.
pub fn enumerate(options: &EnumerateOptions<'_>) -> Catalog {
    let mut diagnostics = Vec::new();
    let mut entries: Vec<Entry> = Vec::new();

    // Highest-ranked tier wins; the sort is stable, so earlier roots win within a tier
    // (which is what makes XDG_DATA_DIRS ordering meaningful).
    let mut ordered = options.roots.clone();
    ordered.sort_by_key(|root| std::cmp::Reverse(root.tier.rank()));

    for root in &ordered {
        for file in card_files_in(&root.path) {
            match parse_card_file(&file, options.lookup) {
                Err(diagnostic) => diagnostics.push(diagnostic),
                Ok((card, expanded)) => {
                    if let Some(existing) = entries.iter_mut().find(|e| e.name == card.name) {
                        // Same name in a lower tier: record the shadowing rather than merging.
                        existing.shadowed.push(ShadowedBy {
                            path: file.clone(),
                            tier: root.tier,
                        });
                        diagnostics.push(Diagnostic {
                            code: DiagnosticCode::Shadowed,
                            path: file,
                            message: format!(
                                "shadowed by {:?} tier card at {}",
                                existing.tier,
                                existing.path.display()
                            ),
                            name: Some(card.name.clone()),
                        });
                        continue;
                    }

                    entries.push(Entry {
                        name: card.name.clone(),
                        version: card.version.clone(),
                        orphaned: is_orphaned(&card),
                        launch_hash: launch_hash(&expanded),
                        card,
                        expanded,
                        tier: root.tier,
                        path: file,
                        shadowed: Vec::new(),
                    });
                }
            }
        }
    }

    entries.sort_by(|a, b| a.name.cmp(&b.name));
    if !options.include_orphaned {
        entries.retain(|e| !e.orphaned);
    }

    Catalog {
        entries,
        diagnostics,
    }
}

/// Read and validate one card file. Never panics: every failure becomes a diagnostic, so a
/// single bad file cannot blank out the catalog.
fn parse_card_file(
    path: &Path,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<(Card, Value), Diagnostic> {
    let text = std::fs::read_to_string(path).map_err(|e| Diagnostic {
        code: DiagnosticCode::Unreadable,
        path: path.to_path_buf(),
        message: e.to_string(),
        name: None,
    })?;

    let value: Value = serde_json::from_str(&text).map_err(|e| Diagnostic {
        code: DiagnosticCode::MalformedJson,
        path: path.to_path_buf(),
        message: e.to_string(),
        name: None,
    })?;

    let card = validate(&value).map_err(|message| Diagnostic {
        code: DiagnosticCode::SchemaInvalid,
        path: path.to_path_buf(),
        message,
        name: value.get("name").and_then(|n| n.as_str()).map(String::from),
    })?;

    // Filename must match `name` — a cheap defense against casual squatting (spec/001 §2).
    let expected = format!("{}{}", card.name, CARD_SUFFIX);
    let actual = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if actual != expected {
        return Err(Diagnostic {
            code: DiagnosticCode::FilenameMismatch,
            path: path.to_path_buf(),
            message: format!("expected {expected}, found {actual}"),
            name: Some(card.name),
        });
    }

    let mut expanded = value;
    expand_card(&mut expanded, lookup);
    let expanded_card = validate(&expanded).map_err(|message| Diagnostic {
        code: DiagnosticCode::SchemaInvalid,
        path: path.to_path_buf(),
        message,
        name: Some(card.name.clone()),
    })?;

    Ok((expanded_card, expanded))
}

/// A card whose launch binary has vanished — typically a failed uninstall (spec/001 §5).
fn is_orphaned(card: &Card) -> bool {
    match card.local.as_ref().and_then(|l| l.launch.as_ref()) {
        Some(launch) => !Path::new(&launch.command).exists(),
        None => false,
    }
}

fn card_files_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return Vec::new(); // a missing registry directory is normal, not an error
    };

    let mut files: Vec<PathBuf> = read_dir
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(CARD_SUFFIX))
        })
        .collect();
    files.sort();
    files
}
