//! Launch hashing — the value consent is bound to (spec/003 §4).

use crate::canonical::canonicalize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// Hash of what would actually execute: the expanded launch stanza plus endpoint.
///
/// Takes the card as a raw `Value` rather than a typed struct so that the bytes hashed are
/// exactly the bytes present in the card, matching the TypeScript implementation, which hashes
/// the parsed object directly. A typed round-trip could add or drop fields and change the digest.
pub fn launch_hash(card: &Value) -> String {
    let local = card.get("local");
    let pick = |key: &str| -> Value {
        local
            .and_then(|l| l.get(key))
            .cloned()
            .unwrap_or(Value::Null)
    };

    let mut subject = Map::new();
    subject.insert("launch".to_string(), pick("launch"));
    subject.insert("endpoint".to_string(), pick("endpoint"));

    let digest = Sha256::digest(canonicalize(&Value::Object(subject)).as_bytes());
    format!("sha256:{:x}", digest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn identity_changes_do_not_affect_the_hash() {
        let base = json!({"name": "a.b", "version": "1.0.0", "description": "x",
                          "local": {"launch": {"type": "stdio", "command": "c"}}});
        let bumped = json!({"name": "a.b", "version": "2.0.0", "description": "x", "title": "New",
                            "local": {"launch": {"type": "stdio", "command": "c"}}});
        assert_eq!(launch_hash(&base), launch_hash(&bumped));
    }

    #[test]
    fn swapping_the_command_does_affect_the_hash() {
        let base = json!({"local": {"launch": {"type": "stdio", "command": "c"}}});
        let swapped = json!({"local": {"launch": {"type": "stdio", "command": "cmd.exe"}}});
        assert_ne!(launch_hash(&base), launch_hash(&swapped));
    }

    #[test]
    fn a_card_with_no_local_block_still_hashes() {
        assert!(launch_hash(&json!({"name": "a.b"})).starts_with("sha256:"));
    }
}
