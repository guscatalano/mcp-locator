//! RFC 8785 (JCS) canonical JSON.
//!
//! This must agree byte-for-byte with `canonicalize()` in the TypeScript client library.
//! Consent binds to a hash of this output (spec/003 §4), so any drift between the two
//! implementations would silently invalidate every stored user decision. The shared test
//! vectors in `conformance/launch-hash.json` are what keep them honest.

use serde_json::Value;
use std::cmp::Ordering;

/// Serialize a value in JCS canonical form.
pub fn canonicalize(value: &Value) -> String {
    let mut out = String::new();
    write_value(value, &mut out);
    out
}

fn write_value(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            // JCS orders keys by UTF-16 code unit. serde_json's Map is already sorted by Rust's
            // byte-wise ordering, which differs from UTF-16 for astral-plane characters, so sort
            // explicitly rather than relying on the map's own order.
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| utf16_cmp(a, b));

            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                write_value(&map[*key], out);
            }
            out.push('}');
        }
    }
}

/// Compare strings by UTF-16 code unit, as JCS requires.
fn utf16_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// JSON string escaping per JCS: the short escapes where they exist, `\u00xx` with lowercase
/// hex for the remaining control characters, and raw UTF-8 for everything else.
fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn orders_keys_and_omits_whitespace() {
        assert_eq!(canonicalize(&json!({"b": 1, "a": 2})), r#"{"a":2,"b":1}"#);
        assert_eq!(
            canonicalize(&json!({"z": [1, 2], "a": {"d": false, "c": null}})),
            r#"{"a":{"c":null,"d":false},"z":[1,2]}"#
        );
    }

    #[test]
    fn escapes_match_the_json_shortcuts() {
        assert_eq!(canonicalize(&json!("say \"hi\"\n")), r#""say \"hi\"\n""#);
        let control = String::from_utf8(vec![1]).unwrap();
        assert_eq!(canonicalize(&Value::String(control)), "\"\\u0001\"");
    }

    #[test]
    fn non_ascii_stays_raw() {
        assert_eq!(canonicalize(&json!("日本語")), "\"日本語\"");
    }
}
