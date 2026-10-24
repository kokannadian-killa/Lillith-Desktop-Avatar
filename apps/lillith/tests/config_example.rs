//! `assets/config.example.toml` must document every config table and key the
//! app reads, with the real defaults.
//!
//! The expected tables and keys are not a hand-kept list: they come from
//! serialising the real defaults of every struct parsed out of config.toml
//! (`lillith_core::config::Config`, `lillith_voice::VoiceConfig` for
//! `[voice]` and `lillith_setup::LlmConfig` for `[llm]`). A nested object is a
//! `[table]`, anything else is a key. Adding a field or table to one of those
//! structs without documenting it in the example fails these tests.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const EXAMPLE: &str = include_str!("../../../assets/config.example.toml");

/// Keys the structs still accept but that nothing reads any more, so the
/// example deliberately leaves them out (`""` = top level).
const UNDOCUMENTED_LEGACY: &[(&str, &str)] = &[
    ("", "llm_enabled"),
    ("", "llm_backend"),
    ("", "llm_url"),
    ("", "llm_model"),
];

/// Optional keys that are skipped when unset, so they don't appear in the
/// serialised defaults but are real keys.
const OPTIONAL_KEYS: &[(&str, &str)] = &[("", "jiggle_mode"), ("", "pose_cycle_s")];

type Key = (String, String);

/// The commented example, parsed.
struct Example {
    /// The example with every `# [table]` / `# key = value` line uncommented.
    /// A key repeated in one table (an alternative value) keeps the first.
    text: String,
    tables: BTreeSet<String>,
    keys: BTreeSet<Key>,
}

fn is_key_line(s: &str) -> Option<&str> {
    let (name, rest) = s.split_once(" = ")?;
    let ok = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    (ok && !rest.is_empty()).then_some(name)
}

fn parse_example() -> Example {
    let mut text = String::new();
    let mut tables = BTreeSet::new();
    let mut keys = BTreeSet::new();
    let mut table = String::new();
    for line in EXAMPLE.lines() {
        let Some(body) = line.strip_prefix("# ") else {
            continue;
        };
        if let Some(rest) = body.strip_prefix('[') {
            let name = rest.split(']').next().unwrap_or_default().trim();
            assert!(!name.is_empty(), "bad table line: {line}");
            assert!(tables.insert(name.to_string()), "[{name}] twice");
            table = name.to_string();
            text.push_str(body);
            text.push('\n');
        } else if let Some(name) = is_key_line(body) {
            if keys.insert((table.clone(), name.to_string())) {
                text.push_str(body);
                text.push('\n');
            }
        }
    }
    Example { text, tables, keys }
}

/// Flattens a serialised struct into its tables and its keys with values.
fn flatten(
    prefix: &str,
    v: &Value,
    tables: &mut BTreeSet<String>,
    keys: &mut BTreeMap<Key, Value>,
) {
    let Value::Object(m) = v else {
        panic!("{prefix} is not a table");
    };
    for (k, v) in m {
        if v.is_object() {
            let path = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{prefix}.{k}")
            };
            tables.insert(path.clone());
            flatten(&path, v, tables, keys);
        } else {
            keys.insert((prefix.to_string(), k.clone()), v.clone());
        }
    }
}

/// Every struct parsed out of config.toml, as one serialised tree.
fn tree(
    core: &lillith_core::config::Config,
    voice: &lillith_voice::VoiceConfig,
    llm: &lillith_setup::LlmConfig,
) -> (BTreeSet<String>, BTreeMap<Key, Value>) {
    let mut root = serde_json::to_value(core).expect("Config serialises");
    let m = root.as_object_mut().expect("Config is a table");
    assert!(!m.contains_key("voice") && !m.contains_key("llm"));
    m.insert("voice".into(), serde_json::to_value(voice).expect("voice"));
    m.insert("llm".into(), serde_json::to_value(llm).expect("llm"));
    let (mut tables, mut keys) = (BTreeSet::new(), BTreeMap::new());
    flatten("", &root, &mut tables, &mut keys);
    (tables, keys)
}

fn defaults() -> (BTreeSet<String>, BTreeMap<Key, Value>) {
    tree(
        &lillith_core::config::Config::default(),
        &lillith_voice::VoiceConfig::default(),
        &lillith_setup::LlmConfig::default(),
    )
}

fn show(k: &Key) -> String {
    if k.0.is_empty() {
        k.1.clone()
    } else {
        format!("[{}] {}", k.0, k.1)
    }
}

#[test]
fn every_config_table_is_in_the_example() {
    let (want, _) = defaults();
    let ex = parse_example();
    let missing: Vec<_> = want.difference(&ex.tables).collect();
    assert!(
        missing.is_empty(),
        "tables missing from config.example.toml: {missing:?}"
    );
    let unknown: Vec<_> = ex.tables.difference(&want).collect();
    assert!(
        unknown.is_empty(),
        "config.example.toml has tables the code doesn't read: {unknown:?}"
    );
    // The ones this example was updated for.
    for t in [
        "mind.access",
        "mind.access.windows",
        "overlay",
        "behavior.tease",
        "voice",
        "llm",
    ] {
        assert!(ex.tables.contains(t), "[{t}]");
    }
}

#[test]
fn every_config_key_is_in_the_example_and_every_example_key_is_real() {
    let (_, want) = defaults();
    let ex = parse_example();
    let legacy: BTreeSet<Key> = UNDOCUMENTED_LEGACY
        .iter()
        .map(|(t, k)| (t.to_string(), k.to_string()))
        .collect();
    let optional: BTreeSet<Key> = OPTIONAL_KEYS
        .iter()
        .map(|(t, k)| (t.to_string(), k.to_string()))
        .collect();
    for k in &legacy {
        assert!(want.contains_key(k), "stale legacy entry {}", show(k));
    }
    let missing: Vec<_> = want
        .keys()
        .filter(|k| !ex.keys.contains(*k) && !legacy.contains(*k))
        .map(show)
        .collect();
    assert!(
        missing.is_empty(),
        "keys missing from config.example.toml: {missing:?}"
    );
    let unknown: Vec<_> = ex
        .keys
        .iter()
        .filter(|k| !want.contains_key(*k) && !optional.contains(*k))
        .map(show)
        .collect();
    assert!(
        unknown.is_empty(),
        "config.example.toml has keys the code doesn't read: {unknown:?}"
    );
}

#[test]
fn example_values_are_the_real_defaults() {
    let ex = parse_example();
    let core = lillith_core::config::Config::from_toml_str(&ex.text)
        .unwrap_or_else(|e| panic!("uncommented example doesn't parse: {e}\n{}", ex.text));
    let voice = lillith_voice::VoiceConfig::from_config_toml(&ex.text).expect("[voice] parses");
    let dir = std::env::temp_dir().join(format!("lillith-config-example-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(&path, &ex.text).unwrap();
    let llm = lillith_setup::config::load(&path);
    let _ = std::fs::remove_dir_all(&dir);
    let llm = llm.expect("[llm] parses and is loopback-only");

    let (_, got) = tree(&core, &voice, &llm);
    let (_, want) = defaults();
    let mut wrong = Vec::new();
    for k in &ex.keys {
        match want.get(k) {
            // No default (an unset Option): the example shows a sample value.
            Some(Value::Null) | None => {}
            Some(d) => {
                if got.get(k) != Some(d) {
                    wrong.push(format!(
                        "{}: example {:?}, default {d}",
                        show(k),
                        got.get(k)
                    ));
                }
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "example values that aren't the defaults: {wrong:#?}"
    );
    // The two keys this example was updated for, spelled out.
    assert!(core.overlay.auto_fix_kwin);
    assert_eq!(
        core.mind.access,
        lillith_core::config::AccessConfig::default()
    );
}
