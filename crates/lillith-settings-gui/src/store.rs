//! Non-destructive edits of `config.toml` for the settings the overlay
//! reads (top-level keys, `[behavior]`, `[springs]`, `[voice]`, `[monitor]`,
//! `[overlay]`).
//!
//! Edits are staged as `key path → value` and applied on [`ConfigStore::save`]
//! onto a *fresh* read of the file (so `[llm]`, written separately by
//! `lillith_setup::config::save`, and anything another program wrote in the
//! meantime is kept). Comments and formatting survive (`toml_edit`).
//! Floats are rounded when staged, so the file never gets `0.6999999881`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Item, Table, Value};

/// A staged value.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Bool(bool),
    Int(i64),
    /// Already rounded.
    Float(f64),
    Str(String),
}

impl Val {
    fn to_value(&self) -> Value {
        match self {
            Val::Bool(b) => Value::from(*b),
            Val::Int(i) => Value::from(*i),
            Val::Float(f) => Value::from(*f),
            Val::Str(s) => Value::from(s.as_str()),
        }
    }
}

/// Rounds to `decimals` places (as `f64`, so the TOML text is short).
pub fn round_to(v: f32, decimals: u32) -> f64 {
    let m = 10f64.powi(decimals as i32);
    // go through the shortest decimal text of the f32 first so 0.7f32
    // becomes 0.7, not 0.699999988079071
    let s: f64 = format!("{v}").parse().unwrap_or(v as f64);
    (s * m).round() / m
}

#[derive(Debug)]
pub struct ConfigStore {
    path: PathBuf,
    doc: DocumentMut,
    edits: BTreeMap<Vec<String>, Val>,
    load_error: Option<String>,
}

fn read_doc(path: &Path) -> Result<DocumentMut, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => s
            .parse::<DocumentMut>()
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DocumentMut::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn lookup<'a>(doc: &'a DocumentMut, path: &[&str]) -> Option<&'a Item> {
    let mut item = doc.as_item();
    for k in path {
        item = item.get(k)?;
    }
    Some(item)
}

impl ConfigStore {
    /// Loads `path` (a missing file is an empty config; a broken one is
    /// reported by [`ConfigStore::load_error`] and treated as empty, and is
    /// never overwritten by `save`).
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let (doc, load_error) = match read_doc(&path) {
            Ok(d) => (d, None),
            Err(e) => (DocumentMut::new(), Some(e)),
        };
        Self {
            path,
            doc,
            edits: BTreeMap::new(),
            load_error,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    fn key(path: &[&str]) -> Vec<String> {
        path.iter().map(|s| s.to_string()).collect()
    }

    fn staged(&self, path: &[&str]) -> Option<&Val> {
        self.edits.get(&Self::key(path))
    }

    pub fn get_bool(&self, path: &[&str]) -> Option<bool> {
        match self.staged(path) {
            Some(Val::Bool(b)) => Some(*b),
            Some(_) => None,
            None => lookup(&self.doc, path)?.as_bool(),
        }
    }

    pub fn get_f32(&self, path: &[&str]) -> Option<f32> {
        match self.staged(path) {
            Some(Val::Float(f)) => Some(*f as f32),
            Some(Val::Int(i)) => Some(*i as f32),
            Some(_) => None,
            None => {
                let it = lookup(&self.doc, path)?;
                it.as_float()
                    .map(|f| f as f32)
                    .or_else(|| it.as_integer().map(|i| i as f32))
            }
        }
    }

    pub fn get_i64(&self, path: &[&str]) -> Option<i64> {
        match self.staged(path) {
            Some(Val::Int(i)) => Some(*i),
            Some(_) => None,
            None => lookup(&self.doc, path)?.as_integer(),
        }
    }

    pub fn get_str(&self, path: &[&str]) -> Option<String> {
        match self.staged(path) {
            Some(Val::Str(s)) => Some(s.clone()),
            Some(_) => None,
            None => lookup(&self.doc, path)?.as_str().map(str::to_string),
        }
    }

    /// Stages a value. Staging the value the file already has un-stages it.
    pub fn set(&mut self, path: &[&str], v: Val) {
        let on_disk = match &v {
            Val::Bool(_) => lookup(&self.doc, path)
                .and_then(|i| i.as_bool())
                .map(Val::Bool),
            Val::Int(_) => lookup(&self.doc, path)
                .and_then(|i| i.as_integer())
                .map(Val::Int),
            Val::Float(_) => lookup(&self.doc, path)
                .and_then(|i| i.as_float())
                .map(Val::Float),
            Val::Str(_) => lookup(&self.doc, path)
                .and_then(|i| i.as_str())
                .map(|s| Val::Str(s.into())),
        };
        if on_disk.as_ref() == Some(&v) {
            self.edits.remove(&Self::key(path));
        } else {
            self.edits.insert(Self::key(path), v);
        }
    }

    pub fn set_bool(&mut self, path: &[&str], b: bool) {
        self.set(path, Val::Bool(b));
    }
    /// Stages a float rounded to `decimals`.
    pub fn set_f32(&mut self, path: &[&str], v: f32, decimals: u32) {
        self.set(path, Val::Float(round_to(v, decimals)));
    }
    pub fn set_i64(&mut self, path: &[&str], v: i64) {
        self.set(path, Val::Int(v));
    }
    pub fn set_str(&mut self, path: &[&str], v: impl Into<String>) {
        self.set(path, Val::Str(v.into()));
    }

    /// Number of staged edits.
    pub fn pending(&self) -> usize {
        self.edits.len()
    }

    pub fn is_pending(&self, path: &[&str]) -> bool {
        self.staged(path).is_some()
    }

    pub fn revert(&mut self) {
        self.edits.clear();
    }

    /// Applies the staged edits onto a fresh read of the file and writes it
    /// atomically (tmp + rename), creating the directory if needed.
    pub fn save(&mut self) -> Result<(), String> {
        let edits = std::mem::take(&mut self.edits);
        self.write(edits)
    }

    /// Like [`ConfigStore::save`], but only the staged edits under `prefix`
    /// (e.g. `["springs", "bounce"]` for a live slider); every other edit
    /// stays staged. `Ok(false)` if there was nothing to write.
    pub fn save_only(&mut self, prefix: &[&str]) -> Result<bool, String> {
        let pre = Self::key(prefix);
        let (mine, rest) = std::mem::take(&mut self.edits)
            .into_iter()
            .partition::<BTreeMap<_, _>, _>(|(k, _)| k.starts_with(&pre));
        self.edits = rest;
        if mine.is_empty() {
            return Ok(false);
        }
        self.write(mine)?;
        Ok(true)
    }

    fn write(&mut self, edits: BTreeMap<Vec<String>, Val>) -> Result<(), String> {
        if edits.is_empty() {
            return Ok(());
        }
        let restore = edits.clone();
        let r = self.write_inner(&edits);
        if r.is_err() {
            self.edits.extend(restore);
        }
        r
    }

    fn write_inner(&mut self, edits: &BTreeMap<Vec<String>, Val>) -> Result<(), String> {
        let mut doc = read_doc(&self.path)?;
        for (path, v) in edits {
            let (last, tables) = path.split_last().expect("non-empty key path");
            let mut t: &mut Table = doc.as_table_mut();
            for k in tables {
                let entry = t.entry(k).or_insert_with(|| {
                    // no empty `[springs]` header above `[springs.bounce]`
                    let mut t = Table::new();
                    t.set_implicit(true);
                    Item::Table(t)
                });
                if !entry.is_table() {
                    // an inline table or scalar in the way: replace with a table
                    *entry = Item::Table(Table::new());
                }
                t = entry.as_table_mut().expect("table");
            }
            match t.get_mut(last).and_then(|i| i.as_value_mut()) {
                Some(old) => {
                    // keep the old value's comments / whitespace
                    let decor = old.decor().clone();
                    *old = v.to_value();
                    *old.decor_mut() = decor;
                }
                None => {
                    t.insert(last, Item::Value(v.to_value()));
                }
            }
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let tmp = self.path.with_extension("toml.settings-tmp");
        std::fs::write(&tmp, doc.to_string()).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("{}: {e}", self.path.display()))?;
        self.doc = doc;
        self.load_error = None;
        Ok(())
    }

    /// Re-reads the file (drops nothing staged).
    pub fn reload(&mut self) {
        if let Ok(d) = read_doc(&self.path) {
            self.doc = d;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "lillith-settings-test-{}-{name}",
            std::process::id()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d.join("config.toml")
    }

    const EXISTING: &str = "# Lillith settings\nmode = \"stay\" # where she lives\nscale = 0.55\n\n[overlay]\nmargin_left = 300\n\n[llm]\n# my model\ngguf_path = \"/m/dolphin.gguf\"\ntemperature = 0.7\n";

    #[test]
    fn round_to_is_short() {
        assert_eq!(round_to(0.7f32, 2), 0.7);
        assert_eq!(round_to(0.123456, 3), 0.123);
        assert_eq!(format!("{}", round_to(0.6f32 + 0.1f32, 2)), "0.7");
    }

    #[test]
    fn round_trip_keeps_other_tables_and_comments() {
        let p = tmp("rt");
        std::fs::write(&p, EXISTING).unwrap();
        let mut s = ConfigStore::load(&p);
        assert_eq!(s.get_str(&["mode"]).as_deref(), Some("stay"));
        assert_eq!(s.get_f32(&["scale"]), Some(0.55));
        assert_eq!(s.get_i64(&["overlay", "margin_left"]), Some(300));
        s.set_str(&["mode"], "wander");
        s.set_f32(&["springs", "jiggle_gain"], 0.6000001, 2);
        s.set_bool(&["voice", "mute"], true);
        s.set_f32(&["scale"], 0.55, 2); // same as disk: not pending
        assert_eq!(s.pending(), 3);
        // someone else writes [llm] in the meantime
        let mut text = std::fs::read_to_string(&p).unwrap();
        text = text.replace("temperature = 0.7", "temperature = 0.9");
        std::fs::write(&p, text).unwrap();
        s.save().unwrap();
        assert_eq!(s.pending(), 0);
        let out = std::fs::read_to_string(&p).unwrap();
        assert!(out.contains("# Lillith settings"));
        assert!(out.contains("mode = \"wander\" # where she lives"), "{out}");
        assert!(out.contains("# my model"));
        assert!(
            out.contains("temperature = 0.9"),
            "fresh read keeps outside edits: {out}"
        );
        assert!(out.contains("jiggle_gain = 0.6\n"), "rounded: {out}");
        assert!(out.contains("[voice]\nmute = true"), "{out}");
        assert!(out.contains("margin_left = 300"));
        // the core config parser still reads it
        let core = lillith_core::config::Config::from_toml_str(&out).unwrap();
        assert_eq!(core.mode, lillith_core::config::MoveMode::Wander);
        assert!((core.springs.jiggle_gain - 0.6).abs() < 1e-6);
        // and the [llm] reader too
        let llm = lillith_setup::config::load(&p).unwrap();
        assert_eq!(llm.gguf_path, "/m/dolphin.gguf");
    }

    #[test]
    fn missing_file_is_created() {
        let p = tmp("new");
        let _ = std::fs::remove_file(&p);
        let mut s = ConfigStore::load(&p);
        assert!(s.load_error().is_none());
        assert_eq!(s.get_bool(&["muted"]), None);
        s.set_bool(&["muted"], true);
        s.set_i64(&["overlay", "margin_bottom"], 48);
        s.save().unwrap();
        let s2 = ConfigStore::load(&p);
        assert_eq!(s2.get_bool(&["muted"]), Some(true));
        assert_eq!(s2.get_i64(&["overlay", "margin_bottom"]), Some(48));
    }

    #[test]
    fn broken_file_is_reported_not_clobbered() {
        let p = tmp("broken");
        std::fs::write(&p, "this is = = not toml").unwrap();
        let mut s = ConfigStore::load(&p);
        assert!(s.load_error().is_some());
        s.set_bool(&["muted"], true);
        assert!(s.save().is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "this is = = not toml");
    }

    #[test]
    fn save_only_writes_one_table_and_keeps_the_rest_staged() {
        let p = tmp("only");
        std::fs::write(&p, EXISTING).unwrap();
        let mut s = ConfigStore::load(&p);
        s.set_str(&["mode"], "wander");
        s.set_f32(&["springs", "bounce", "intensity"], 2.5, 2);
        assert_eq!(s.save_only(&["springs", "bounce"]), Ok(true));
        assert_eq!(s.save_only(&["springs", "bounce"]), Ok(false));
        assert_eq!(s.pending(), 1, "mode still staged");
        let out = std::fs::read_to_string(&p).unwrap();
        assert!(out.contains("[springs.bounce]\nintensity = 2.5"), "{out}");
        assert!(out.contains("mode = \"stay\""), "{out}");
        let core = lillith_core::config::Config::from_toml_str(&out).unwrap();
        assert_eq!(core.springs.bounce.intensity, 2.5);
    }

    #[test]
    fn access_switches_land_in_mind_access() {
        let p = tmp("access");
        let _ = std::fs::remove_file(&p);
        let mut s = ConfigStore::load(&p);
        s.set_bool(&["mind", "access", "commands"], false);
        s.set_bool(&["mind", "access", "import_desktop_skills"], false);
        s.set_str(&["mind", "skills_dir"], "/home/ko/skills");
        s.save().unwrap();
        let cfg =
            lillith_core::config::Config::from_toml_str(&std::fs::read_to_string(&p).unwrap())
                .unwrap();
        assert!(!cfg.mind.access.commands);
        assert!(!cfg.mind.access.import_desktop_skills);
        assert!(cfg.mind.access.files, "untouched switches stay on");
        assert_eq!(cfg.mind.skills_dir, "/home/ko/skills");
    }
}
