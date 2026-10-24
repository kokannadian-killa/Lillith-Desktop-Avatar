//! `LEARN` audit lines (docs/SELF_LEARNING.md §2):
//! `<ts>\tLEARN\t<op>\t<topic>\tnote=<id>`. Never the note text.

use std::path::Path;

/// `YYYY-MM-DDTHH:MM:SSZ` for Unix seconds `t`.
pub fn utc(t: f64) -> String {
    let secs = t.max(0.0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn col(s: &str) -> String {
    lillith_core::launch_audit::one_column(s)
        .chars()
        .take(64)
        .collect()
}

/// One audit line (with the trailing newline).
pub fn line(t: f64, op: &str, topic: &str, note_id: &str) -> String {
    format!(
        "{}\tLEARN\t{}\t{}\tnote={}\n",
        utc(t),
        col(op),
        col(topic),
        col(note_id)
    )
}

/// An online research trip line (§5.1, §5.3):
/// `<ts>\t<STATE>\tonline_research\tcard=<id>\t<field>\t…`. Each field is
/// one column (tabs and newlines flattened), cut to 2048 chars so a long URL
/// list can't swamp the log.
pub fn online_line(t: f64, state: &str, card: &str, fields: &[String]) -> String {
    let mut l = format!(
        "{}\t{}\t{}\tcard={}",
        utc(t),
        col(state),
        lillith_core::soul_contract::ONLINE_RESEARCH_KIND,
        col(card)
    );
    for f in fields {
        l.push('\t');
        l.extend(lillith_core::launch_audit::one_column(f).chars().take(2048));
    }
    l.push('\n');
    l
}

/// Appends an online research trip line to `log` (0600 when created).
/// Fail-closed: no log, or a failed write, is an error the trip engine acts
/// on (a trip whose `PROPOSED` line can't be written never starts).
pub fn record_online(
    log: Option<&Path>,
    state: &str,
    card: &str,
    fields: &[String],
) -> std::io::Result<()> {
    let Some(log) = log else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no audit log path",
        ));
    };
    write_line(log, &online_line(crate::now_s(), state, card, fields))
}

/// Appends a helper-worker line (`<ts>\tWORKER\t<op>\t<reason>\tjob=<id>`,
/// docs §12) to `log`.
pub fn record_worker(log: Option<&Path>, op: &str, reason: &str, job: &str) -> std::io::Result<()> {
    let Some(log) = log else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no audit log path",
        ));
    };
    write_line(
        log,
        &crate::workers::audit_line(crate::now_s(), op, reason, job),
    )
}

/// Appends a `LEARN` line to `log` (file 0600 when created). Failures are
/// logged, never fatal.
pub fn record(log: Option<&Path>, op: &str, topic: &str, note_id: &str) {
    append(log, &line(crate::now_s(), op, topic, note_id));
}

fn append(log: Option<&Path>, l: &str) {
    let Some(log) = log else { return };
    if let Err(e) = write_line(log, l) {
        tracing::warn!("learn audit: {e}");
    }
}

/// Appends `l` to `log` (created 0600, parents made), synced to disk.
fn write_line(log: &Path, l: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(d) = log.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut o = std::fs::OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(log)?;
    f.write_all(l.as_bytes())?;
    f.sync_data()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_and_line_shape() {
        assert_eq!(utc(0.0), "1970-01-01T00:00:00Z");
        assert_eq!(utc(1_792_800_000.0), "2026-10-24T00:00:00Z");
        assert_eq!(
            line(0.0, "write", "rust\tx", "n-1"),
            "1970-01-01T00:00:00Z\tLEARN\twrite\trust x\tnote=n-1\n"
        );
        assert_eq!(
            online_line(
                0.0,
                "PROPOSED",
                "c-1",
                &["pages=5".into(), "a\tb\nc".into()]
            ),
            "1970-01-01T00:00:00Z\tPROPOSED\tonline_research\tcard=c-1\tpages=5\ta b c\n"
        );
    }
}
