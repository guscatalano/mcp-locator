//! Card model, validation, and environment expansion (spec/001 §3).
//!
//! Validation is hand-written against the same rules as the JSON Schema in
//! `packages/schema`, rather than embedding a JSON Schema engine: the broker is a small trusted
//! binary and the schema validator would dominate its dependency tree. The conformance fixtures
//! are what keep the two rule sets from drifting — every diagnostic code is exercised there.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LaunchType {
    Stdio,
    Executable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EndpointType {
    Pipe,
    UnixSocket,
    StreamableHttp,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Launch {
    #[serde(rename = "type")]
    pub launch_type: LaunchType,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Endpoint {
    #[serde(rename = "type")]
    pub endpoint_type: EndpointType,
    pub address: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Liveness {
    #[serde(default, rename = "pidFile", skip_serializing_if = "Option::is_none")]
    pub pid_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Lifetime {
    #[serde(
        default,
        rename = "idleTimeoutSeconds",
        skip_serializing_if = "Option::is_none"
    )]
    pub idle_timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct LocalBlock {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<Launch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<Endpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub liveness: Option<Liveness>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifetime: Option<Lifetime>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Card {
    pub name: String,
    pub version: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<LocalBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remotes: Option<Vec<Value>>,
}

/// Why a card file did not yield a catalog entry. Mirrors the TypeScript `DiagnosticCode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosticCode {
    MalformedJson,
    SchemaInvalid,
    FilenameMismatch,
    Unreadable,
    Shadowed,
}

/// Validate a parsed card against the spec/001 §3 rules. Returns the reason it is invalid.
pub fn validate(value: &Value) -> Result<Card, String> {
    let obj = value.as_object().ok_or("card must be a JSON object")?;

    for required in ["name", "version", "description"] {
        if !obj.contains_key(required) {
            return Err(format!("missing required field `{required}`"));
        }
    }

    let name = obj["name"].as_str().ok_or("`name` must be a string")?;
    if !is_reverse_dns(name) {
        return Err(format!("`name` is not a reverse-DNS identifier: {name}"));
    }

    let card: Card = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;

    let has_launch = card
        .local
        .as_ref()
        .and_then(|l| l.launch.as_ref())
        .is_some();
    let has_endpoint = card
        .local
        .as_ref()
        .and_then(|l| l.endpoint.as_ref())
        .is_some();
    let has_remotes = card.remotes.as_ref().is_some_and(|r| !r.is_empty());

    if !has_launch && !has_endpoint && !has_remotes {
        return Err("card declares no launch, endpoint, or remotes".to_string());
    }

    // An `executable` launch starts a process that then serves on its own endpoint, so the
    // endpoint has to be declared — there is nothing to connect to otherwise.
    if let Some(launch) = card.local.as_ref().and_then(|l| l.launch.as_ref()) {
        if launch.launch_type == LaunchType::Executable && !has_endpoint {
            return Err("`executable` launch requires `local.endpoint`".to_string());
        }
    }

    Ok(card)
}

fn is_reverse_dns(name: &str) -> bool {
    let labels: Vec<&str> = name.split('.').collect();
    if labels.len() < 2 || name.len() > 200 {
        return false;
    }
    labels.iter().all(|label| {
        !label.is_empty()
            && label
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    })
}

/// Expand `%VAR%` and `${VAR}` references. Both forms are accepted on every platform so one
/// card file stays portable; unknown variables are left verbatim, because silently emptying a
/// path would turn a typo into a launch of the wrong file (spec/001 §3).
pub fn expand_env(input: &str, lookup: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let (opener, closer, skip) = match (chars[i], chars.get(i + 1)) {
            ('%', _) => ('%', '%', 1),
            ('$', Some('{')) => ('{', '}', 2),
            _ => {
                out.push(chars[i]);
                i += 1;
                continue;
            }
        };
        let _ = opener;

        match find_var(&chars, i + skip, closer) {
            Some((name, end)) if is_var_name(&name) => match lookup(&name) {
                Some(value) => {
                    out.push_str(&value);
                    i = end + 1;
                }
                None => {
                    out.extend(&chars[i..=end]);
                    i = end + 1;
                }
            },
            _ => {
                out.push(chars[i]);
                i += 1;
            }
        }
    }
    out
}

fn find_var(chars: &[char], start: usize, closer: char) -> Option<(String, usize)> {
    let end = chars[start..].iter().position(|&c| c == closer)? + start;
    Some((chars[start..end].iter().collect(), end))
}

fn is_var_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Expand env references in every field that resolves to a filesystem or endpoint location.
/// Operates on the raw `Value` so the hashed bytes stay identical to the TypeScript
/// implementation, which expands the parsed object in place.
pub fn expand_card(value: &mut Value, lookup: &dyn Fn(&str) -> Option<String>) {
    let Some(local) = value.get_mut("local").and_then(|l| l.as_object_mut()) else {
        return;
    };

    if let Some(launch) = local.get_mut("launch").and_then(|l| l.as_object_mut()) {
        expand_string_field(launch.get_mut("command"), lookup);
        expand_string_field(launch.get_mut("cwd"), lookup);
        if let Some(args) = launch.get_mut("args").and_then(|a| a.as_array_mut()) {
            for arg in args.iter_mut() {
                expand_string_field(Some(arg), lookup);
            }
        }
        if let Some(env) = launch.get_mut("env").and_then(|e| e.as_object_mut()) {
            for (_, v) in env.iter_mut() {
                expand_string_field(Some(v), lookup);
            }
        }
    }
    if let Some(endpoint) = local.get_mut("endpoint").and_then(|e| e.as_object_mut()) {
        expand_string_field(endpoint.get_mut("address"), lookup);
    }
    if let Some(liveness) = local.get_mut("liveness").and_then(|l| l.as_object_mut()) {
        expand_string_field(liveness.get_mut("pidFile"), lookup);
    }
}

fn expand_string_field(field: Option<&mut Value>, lookup: &dyn Fn(&str) -> Option<String>) {
    if let Some(Value::String(s)) = field {
        *s = expand_env(s, lookup);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name: &str| {
            owned
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        }
    }

    #[test]
    fn both_env_syntaxes_expand() {
        let lookup = env(&[("FOO", "bar")]);
        assert_eq!(expand_env("%FOO%/x", &lookup), "bar/x");
        assert_eq!(expand_env("${FOO}/x", &lookup), "bar/x");
    }

    #[test]
    fn unknown_variables_are_left_verbatim() {
        let lookup = env(&[]);
        assert_eq!(expand_env("%NOPE%/x", &lookup), "%NOPE%/x");
        assert_eq!(expand_env("${NOPE}/x", &lookup), "${NOPE}/x");
    }

    #[test]
    fn stray_percent_is_not_treated_as_a_variable() {
        let lookup = env(&[]);
        assert_eq!(expand_env("50% done", &lookup), "50% done");
        assert_eq!(
            expand_env("C:\\plain\\path.exe", &lookup),
            "C:\\plain\\path.exe"
        );
    }

    #[test]
    fn executable_launch_requires_an_endpoint() {
        let card = json!({"name": "a.b", "version": "1", "description": "x",
                          "local": {"launch": {"type": "executable", "command": "c"}}});
        assert!(validate(&card).is_err());
    }

    #[test]
    fn a_card_with_nothing_to_connect_to_is_invalid() {
        let card = json!({"name": "a.b", "version": "1", "description": "x"});
        assert!(validate(&card).is_err());
    }

    #[test]
    fn name_must_be_reverse_dns() {
        assert!(is_reverse_dns("com.example.app"));
        assert!(!is_reverse_dns("noDots"));
        assert!(!is_reverse_dns("com.Example.App"));
        assert!(!is_reverse_dns("com..app"));
    }
}
