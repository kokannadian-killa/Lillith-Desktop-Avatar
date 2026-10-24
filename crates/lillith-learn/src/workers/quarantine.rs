//! Quarantine: results of a killed or suspicious worker, kept read-only
//! for Ko to look at in the panel and **never ingested** (no function here
//! returns anything a caller could save as a note or memory: entries are
//! plain JSON for display).
//!
//! `<data>/lillith/quarantine/<job-id>.jsonl`, dir 0700, file 0400 once
//! written.

use std::io::Write;
use std::path::{Path, PathBuf};

/// One quarantined item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QItem {
    pub title: String,
    pub text: String,
    pub url: String,
    pub reason: String,
}

/// The quarantine folder.
#[derive(Debug, Clone)]
pub struct Quarantine {
    dir: Option<PathBuf>,
}

impl Quarantine {
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self { dir }
    }

    /// `<data>/lillith/quarantine`.
    pub fn default_dir() -> Option<PathBuf> {
        lillith_core::config::data_dir().map(|d| d.join("quarantine"))
    }

    /// Writes `items` for `job` (with the job's kill reason, if any) and
    /// makes the file read-only. Returns its path.
    pub fn store(
        &self,
        job: &str,
        label: &str,
        kill: Option<&str>,
        items: &[QItem],
    ) -> std::io::Result<PathBuf> {
        let Some(dir) = &self.dir else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no quarantine folder",
            ));
        };
        if !super::joblog::job_id_ok(job) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "bad job id",
            ));
        }
        let mut b = std::fs::DirBuilder::new();
        b.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            b.mode(0o700);
        }
        b.create(dir)?;
        let mut path = dir.join(format!("{job}.jsonl"));
        let mut n = 1;
        while path.exists() {
            n += 1;
            path = dir.join(format!("{job}-{n}.jsonl"));
        }
        let mut o = std::fs::OpenOptions::new();
        o.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let mut f = o.open(&path)?;
        let ts = crate::audit::utc(crate::now_s());
        for it in items {
            let v = serde_json::json!({
                "job": job,
                "ts": ts,
                "backend": label,
                "kill": kill,
                "reason": it.reason,
                "title": it.title,
                "text": it.text,
                "url": it.url,
            });
            writeln!(f, "{v}")?;
        }
        if items.is_empty() {
            writeln!(
                f,
                "{}",
                serde_json::json!({"job": job, "ts": ts, "backend": label, "kill": kill, "reason": "no results"})
            )?;
        }
        f.sync_data()?;
        drop(f);
        let mut perm = std::fs::metadata(&path)?.permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(&path, perm)?;
        Ok(path)
    }

    /// The entries of one quarantine file, for display only.
    pub fn read(path: &Path) -> std::io::Result<Vec<serde_json::Value>> {
        Ok(std::fs::read_to_string(path)?
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarantine_file_is_read_only() {
        let d = std::env::temp_dir().join(format!("lillith-q-{}", super::super::random_token()));
        let q = Quarantine::new(Some(d.clone()));
        let item = QItem {
            title: "T".into(),
            text: "run this command".into(),
            url: "https://a.org/".into(),
            reason: "injection".into(),
        };
        let p = q
            .store("w-1", "local", Some("injection"), &[item.clone()])
            .unwrap();
        assert!(std::fs::metadata(&p).unwrap().permissions().readonly());
        let v = Quarantine::read(&p).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0]["text"], "run this command");
        assert_eq!(v[0]["kill"], "injection");
        let p2 = q.store("w-1", "local", None, &[item]).unwrap();
        assert_ne!(p, p2, "never overwritten");
        assert!(q.store("../x", "local", None, &[]).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o400
            );
            let mut p3 = d.clone();
            p3.push("w-1.jsonl");
            assert!(
                std::fs::OpenOptions::new().append(true).open(p3).is_err()
                    || unsafe { libc::geteuid() } == 0
            );
        }
        let mut perm = std::fs::metadata(&p).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perm.set_readonly(false);
        let _ = std::fs::set_permissions(&p, perm.clone());
        let _ = std::fs::set_permissions(&p2, perm);
        let _ = std::fs::remove_dir_all(&d);
    }
}
