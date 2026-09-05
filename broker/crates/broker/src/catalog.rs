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
                        orphaned: is_orphaned(&card, options.lookup),
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

    // Strip a UTF-8 BOM: see the matching note in parse.ts. The two parsers have to agree on
    // what counts as a valid card, so this is not a place to be stricter than the other.
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let value: Value = serde_json::from_str(text).map_err(|e| Diagnostic {
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
fn is_orphaned(card: &Card, lookup: &dyn Fn(&str) -> Option<String>) -> bool {
    match card.local.as_ref().and_then(|l| l.launch.as_ref()) {
        Some(launch) => resolve_command(&launch.command, lookup).is_none(),
        None => false,
    }
}

/// Resolve a launch command to a file on disk, searching PATH for a bare name.
///
/// Mirrors `command.ts`; see the note there for why a bare `node` must not read as orphaned.
/// The rule is the one the OS itself applies at launch: a command containing a separator is a
/// path, anything else is a name to look up. PATH comes through `lookup` rather than the real
/// environment so the conformance fixtures drive both implementations from identical inputs.
pub fn resolve_command(command: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let windows = cfg!(windows);
    let has_separator = command.contains('/') || (windows && command.contains('\\'));
    if has_separator {
        let path = PathBuf::from(command);
        return path.exists().then_some(path);
    }

    // Windows resolves a bare name against PATHEXT, so `node` finds `node.exe`.
    let extensions: Vec<String> = if windows {
        let pathext = lookup("PATHEXT").unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_string());
        std::iter::once(String::new())
            .chain(
                pathext
                    .split(';')
                    .filter(|e| !e.is_empty())
                    .map(String::from),
            )
            .collect()
    } else {
        vec![String::new()]
    };

    let separator = if windows { ';' } else { ':' };
    let path_var = lookup("PATH")
        .or_else(|| lookup("Path"))
        .unwrap_or_default();
    for dir in path_var.split(separator).filter(|d| !d.is_empty()) {
        for extension in &extensions {
            let candidate = Path::new(dir).join(format!("{command}{extension}"));
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A program every machine of this platform has, referred to by bare name.
    const WELL_KNOWN: &str = if cfg!(windows) { "cmd" } else { "sh" };

    fn real_env(name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    #[test]
    fn a_bare_command_is_resolved_through_path() {
        // The case that mattered in practice: a card saying `"command": "node"` is more portable
        // than one naming an absolute path, and used to be hidden as an orphan with no
        // diagnostic. Must agree with `command.ts`, which has the same test.
        assert!(resolve_command(WELL_KNOWN, &real_env).is_some());
        assert!(resolve_command("mcp-locator-definitely-not-installed", &real_env).is_none());
    }

    #[test]
    fn a_command_with_a_separator_is_a_path_and_is_never_searched() {
        // Otherwise a missing `./tools/notes.exe` could be satisfied by an unrelated `notes.exe`
        // earlier on PATH — the wrong program, launched under a name the user already approved.
        let relative = format!("./{WELL_KNOWN}");
        assert!(resolve_command(&relative, &real_env).is_none());
    }

    #[test]
    fn path_comes_from_the_supplied_lookup_not_the_process() {
        let empty = |_: &str| Some(String::new());
        assert!(resolve_command(WELL_KNOWN, &empty).is_none());
    }
}
