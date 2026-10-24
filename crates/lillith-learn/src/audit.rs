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

/// Appends a `LEARN` line to `log` (file 0600 when created). Failures are
/// logged, never fatal.
pub fn record(log: Option<&Path>, op: &str, topic: &str, note_id: &str) {
    let Some(log) = log else { return };
    let l = line(crate::now_s(), op, topic, note_id);
    let res = (|| -> std::io::Result<()> {
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
        o.open(log)?.write_all(l.as_bytes())
    })();
    if let Err(e) = res {
        tracing::warn!("learn audit: {e}");
    }
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
    }
}
