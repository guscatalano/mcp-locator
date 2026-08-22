//! The broker's catalog must produce the same result as the TypeScript client library.
//!
//! Both read `conformance/fixtures/basic` and are compared against
//! `conformance/expected/basic.json`. A failure here means the two implementations disagree
//! about what is registered on the machine — fix the implementation, not the fixture.

use mcp_locator_broker::catalog::{enumerate, Catalog, EnumerateOptions};
use mcp_locator_broker::dirs::{Root, Tier};
use serde_json::Value;
use std::path::PathBuf;

fn conformance_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .join("conformance")
}

fn fixture_root(name: &str) -> PathBuf {
    conformance_dir().join("fixtures").join(name)
}

fn fixture_roots(name: &str) -> Vec<Root> {
    let base = fixture_root(name);
    vec![
        Root {
            tier: Tier::System,
            path: base.join("system"),
        },
        Root {
            tier: Tier::User,
            path: base.join("user"),
        },
        Root {
            tier: Tier::Low,
            path: base.join("low"),
        },
    ]
}

fn expected() -> Value {
    let raw = std::fs::read_to_string(conformance_dir().join("expected").join("basic.json"))
        .expect("conformance/expected/basic.json must exist");
    serde_json::from_str(&raw).expect("expected output must be valid JSON")
}

fn run(include_orphaned: bool) -> Catalog {
    let base = fixture_root("basic").to_string_lossy().to_string();
    // Fixture cards reference ${FIXTURE_ROOT}; supply it explicitly rather than mutating the
    // process environment, which would race across parallel tests.
    let lookup = move |name: &str| {
        if name == "FIXTURE_ROOT" {
            Some(base.clone())
        } else {
            std::env::var(name).ok()
        }
    };
    enumerate(&EnumerateOptions {
        roots: fixture_roots("basic"),
        lookup: &lookup,
        include_orphaned,
    })
}

/// Reduce to the same projection the TypeScript tests compare: absolute paths differ per
/// machine and OS, so only the platform-independent shape is asserted.
fn normalize(catalog: &Catalog) -> (Vec<Value>, Vec<Value>) {
    let base = fixture_root("basic");
    let strip = |path: &str| -> Value {
        let relative = path
            .strip_prefix(&*base.to_string_lossy())
            .unwrap_or(path)
            .trim_start_matches(['\\', '/'])
            .replace('\\', "/");
        Value::String(relative)
    };

    let entries = catalog
        .entries
        .iter()
        .map(|e| {
            serde_json::json!({
                "name": e.name,
                "tier": e.tier,
                "version": e.version,
                "orphaned": e.orphaned,
                "shadowedTiers": e.shadowed.iter().map(|s| s.tier).collect::<Vec<_>>(),
                "command": match e.card.local.as_ref().and_then(|l| l.launch.as_ref()) {
                    Some(launch) => strip(&launch.command),
                    None => Value::Null,
                },
            })
        })
        .collect();

    let mut diagnostics: Vec<Value> = catalog
        .diagnostics
        .iter()
        .map(|d| {
            serde_json::json!({
                "code": d.code,
                "name": d.name.clone().map(Value::String).unwrap_or(Value::Null),
            })
        })
        .collect();
    diagnostics.sort_by_key(|d| {
        (
            d["code"].as_str().unwrap_or_default().to_string(),
            d["name"].as_str().unwrap_or_default().to_string(),
        )
    });

    (entries, diagnostics)
}

#[test]
fn merged_catalog_matches_the_shared_expectation() {
    let catalog = run(true);
    let (entries, diagnostics) = normalize(&catalog);
    let expected = expected();

    assert_eq!(
        Value::Array(entries),
        expected["entries"],
        "catalog entries differ from the TypeScript implementation"
    );
    assert_eq!(
        Value::Array(diagnostics),
        expected["diagnostics"],
        "diagnostics differ from the TypeScript implementation"
    );
}

#[test]
fn orphaned_cards_are_hidden_unless_requested() {
    let catalog = run(false);
    let visible: Vec<&str> = catalog.entries.iter().map(|e| e.name.as_str()).collect();
    let expected = expected();
    let want: Vec<&str> = expected["visibleByDefault"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();

    assert_eq!(visible, want);
}

#[test]
fn higher_tier_shadows_lower() {
    let catalog = run(false);
    let entry = catalog
        .entries
        .iter()
        .find(|e| e.name == "com.example.shadowed")
        .expect("shadowed entry must be present");

    assert_eq!(entry.tier, Tier::System);
    assert_eq!(entry.version, "2.0.0", "system-tier card must win");
    assert_eq!(entry.shadowed.len(), 1);
    assert_eq!(entry.shadowed[0].tier, Tier::User);
}

#[test]
fn a_malformed_card_does_not_blank_the_catalog() {
    let catalog = run(false);
    assert!(catalog.entries.len() >= 4);
    assert!(catalog
        .diagnostics
        .iter()
        .any(|d| matches!(d.code, mcp_locator_proto::DiagnosticCode::MalformedJson)));
}

#[test]
fn missing_registry_directories_are_not_an_error() {
    let lookup = |name: &str| std::env::var(name).ok();
    let catalog = enumerate(&EnumerateOptions {
        roots: vec![Root {
            tier: Tier::User,
            path: conformance_dir().join("does-not-exist"),
        }],
        lookup: &lookup,
        include_orphaned: false,
    });
    assert!(catalog.entries.is_empty());
    assert!(catalog.diagnostics.is_empty());
}
