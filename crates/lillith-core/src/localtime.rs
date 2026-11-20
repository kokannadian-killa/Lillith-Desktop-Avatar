//! Wall-clock labels in the system's local time zone ("7:30 PM MDT"), for
//! what she *says* (e.g. the dream deadline). Audit lines stay UTC.
//!
//! Same facility as [`crate::seasons`]: `localtime_r` on Unix,
//! `GetLocalTime` / `GetSystemTime` on Windows. No time-zone crate.

/// `unix_secs` as a local clock label: `7:30 PM MDT`, with ` tomorrow`
/// when that is on the next local day. Falls back to `HH:MM UTC` when the
/// local time can't be read.
pub fn clock_label(unix_secs: i64) -> String {
    match local_parts(unix_secs) {
        Some(p) => format_clock(p.hour, p.minute, p.zone.as_deref(), p.days_ahead),
        None => {
            let s = unix_secs.rem_euclid(86_400);
            format!("{:02}:{:02} UTC", s / 3600, (s % 3600) / 60)
        }
    }
}

/// A `YYYY-MM-DDTHH:MM:SSZ` UTC stamp (the audit / `deadline` format)
/// as a local clock label ([`clock_label`]); `None` when it isn't one.
pub fn label_for_utc_stamp(stamp: &str) -> Option<String> {
    parse_utc_stamp(stamp).map(clock_label)
}

/// Seconds since the epoch of a `YYYY-MM-DDTHH:MM:SSZ` stamp.
pub fn parse_utc_stamp(stamp: &str) -> Option<i64> {
    let b = stamp.trim().as_bytes();
    if b.len() != 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
        || b[19] != b'Z'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let s = std::str::from_utf8(&b[r]).ok()?;
        s.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| s.parse().ok())?
    };
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, se) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    Some(days_from_civil(y, mo as u32, d as u32) * 86_400 + h * 3600 + mi * 60 + se)
}

/// A zone label from an offset east of UTC: `UTC-06:00`, `UTC+05:30`,
/// `UTC` (where the platform has no abbreviation).
pub fn offset_label(offset_secs: i64) -> String {
    let m = offset_secs / 60;
    if m == 0 {
        return "UTC".into();
    }
    let sign = if m < 0 { '-' } else { '+' };
    format!("UTC{sign}{:02}:{:02}", m.abs() / 60, m.abs() % 60)
}

/// A local wall-clock time, broken down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalParts {
    pub hour: u32,
    pub minute: u32,
    /// Zone abbreviation (`MDT`) where the platform has one.
    pub zone: Option<String>,
    /// Local calendar days after today (0 = today).
    pub days_ahead: i64,
}

/// `7:30 PM MDT` (12-hour clock; `tomorrow` / `in N days` when not today).
/// Pure, for tests.
pub fn format_clock(hour: u32, minute: u32, zone: Option<&str>, days_ahead: i64) -> String {
    let (h12, ampm) = match hour % 24 {
        0 => (12, "AM"),
        h @ 1..=11 => (h, "AM"),
        12 => (12, "PM"),
        h => (h - 12, "PM"),
    };
    let mut s = format!("{h12}:{minute:02} {ampm}");
    if let Some(z) = zone.map(str::trim).filter(|z| !z.is_empty()) {
        s.push(' ');
        s.push_str(z);
    }
    match days_ahead {
        d if d <= 0 => {}
        1 => s.push_str(" tomorrow"),
        d => s.push_str(&format!(" in {d} days")),
    }
    s
}

/// `unix_secs` broken down in the local zone (`None`: can't read it).
#[cfg(unix)]
pub fn local_parts(unix_secs: i64) -> Option<LocalParts> {
    fn tm_of(t: libc::time_t) -> Option<libc::tm> {
        // SAFETY: `localtime_r` with valid pointers to locals.
        unsafe {
            let mut tm: libc::tm = std::mem::zeroed();
            if libc::localtime_r(&t, &mut tm).is_null() {
                return None;
            }
            Some(tm)
        }
    }
    let t = libc::time_t::try_from(unix_secs).ok()?;
    let tm = tm_of(t)?;
    // SAFETY: `time(NULL)` only returns the time.
    let now = tm_of(unsafe { libc::time(std::ptr::null_mut()) })?;
    let day = |tm: &libc::tm| i64::from(tm.tm_year) * 400 + i64::from(tm.tm_yday);
    let days_ahead = if tm.tm_year == now.tm_year {
        i64::from(tm.tm_yday - now.tm_yday)
    } else {
        // Across New Year: only "tomorrow" matters for a session.
        i64::from(day(&tm) > day(&now))
    };
    Some(LocalParts {
        hour: u32::try_from(tm.tm_hour).ok().filter(|h| *h < 24)?,
        minute: u32::try_from(tm.tm_min).ok().filter(|m| *m < 60)?,
        zone: zone_of(&tm),
        days_ahead,
    })
}

#[cfg(all(
    unix,
    any(target_os = "linux", target_os = "android", target_os = "macos")
))]
#[allow(clippy::useless_conversion)] // `c_long` is `i64` on some targets only
fn zone_of(tm: &libc::tm) -> Option<String> {
    if tm.tm_zone.is_null() {
        return Some(offset_label(i64::from(tm.tm_gmtoff)));
    }
    // SAFETY: a non-null `tm_zone` from `localtime_r` points at a
    // NUL-terminated static string.
    let z = unsafe { std::ffi::CStr::from_ptr(tm.tm_zone) };
    let z = z.to_str().ok().map(str::trim).unwrap_or_default();
    let ok = !z.is_empty()
        && z.len() <= 8
        && z.chars()
            .all(|c| c.is_ascii_alphanumeric() || "+-".contains(c));
    // No usable abbreviation: the numeric offset, so there is always a zone.
    Some(if ok {
        z.to_string()
    } else {
        offset_label(i64::from(tm.tm_gmtoff))
    })
}

#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "android", target_os = "macos"))
))]
fn zone_of(_: &libc::tm) -> Option<String> {
    None
}

/// Windows: the current local offset (local wall clock minus UTC) applied
/// to `unix_secs` (a DST change inside the session is not modelled).
#[cfg(windows)]
pub fn local_parts(unix_secs: i64) -> Option<LocalParts> {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
    fn secs(st: &SYSTEMTIME) -> i64 {
        days_from_civil(
            i64::from(st.wYear),
            u32::from(st.wMonth),
            u32::from(st.wDay),
        ) * 86_400
            + i64::from(st.wHour) * 3600
            + i64::from(st.wMinute) * 60
            + i64::from(st.wSecond)
    }
    // SAFETY: both only write the SYSTEMTIME we pass.
    let mut l: SYSTEMTIME = unsafe { std::mem::zeroed() };
    let mut u: SYSTEMTIME = unsafe { std::mem::zeroed() };
    // SAFETY: as above.
    unsafe {
        GetLocalTime(&mut l);
        GetSystemTime(&mut u);
    }
    // Rounded to the quarter hour (the two reads are not atomic).
    let offset = ((secs(&l) - secs(&u)) as f64 / 900.0).round() as i64 * 900;
    let local = unix_secs + offset;
    let now_local = secs(&u) + offset;
    let s = local.rem_euclid(86_400);
    Some(LocalParts {
        hour: (s / 3600) as u32,
        minute: ((s % 3600) / 60) as u32,
        // Windows has no short abbreviation API: the numeric offset.
        zone: Some(offset_label(offset)),
        days_ahead: local.div_euclid(86_400) - now_local.div_euclid(86_400),
    })
}

/// Days since 1970-01-01 of a civil date (Howard Hinnant).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(not(any(unix, windows)))]
pub fn local_parts(_: i64) -> Option<LocalParts> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_a_twelve_hour_clock_with_zone_and_day() {
        assert_eq!(format_clock(19, 30, Some("MDT"), 0), "7:30 PM MDT");
        assert_eq!(format_clock(0, 5, None, 1), "12:05 AM tomorrow");
        assert_eq!(format_clock(12, 0, Some(" "), 0), "12:00 PM");
        assert_eq!(format_clock(9, 7, Some("CET"), 2), "9:07 AM CET in 2 days");
    }

    #[test]
    fn labels_are_local_clock_times_not_utc_stamps() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let p = local_parts(now + 3600).expect("local time readable");
        assert!(p.hour < 24 && p.minute < 60);
        assert!((0..=1).contains(&p.days_ahead));
        let l = clock_label(now + 3600);
        assert!(l.contains("AM") || l.contains("PM"), "{l}");
        assert!(!l.ends_with('Z'), "{l}");
    }

    #[test]
    fn civil_days() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2026, 10, 30), 20_756);
    }

    #[test]
    fn utc_stamps_parse_and_label_locally() {
        assert_eq!(parse_utc_stamp("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_utc_stamp("2026-10-31T01:30:00Z"),
            Some(20_757 * 86_400 + 5_400)
        );
        for bad in [
            "",
            "2026-10-31 01:30:00",
            "2026-13-01T00:00:00Z",
            "2026-10-31T01:30:00+00",
        ] {
            assert_eq!(parse_utc_stamp(bad), None, "{bad}");
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let p = local_parts(now).expect("local time readable");
        assert!(p.zone.is_some(), "a zone label is always there");
        let l = label_for_utc_stamp("2026-10-31T01:30:00Z").unwrap();
        assert_eq!(l, clock_label(20_757 * 86_400 + 5_400));
        assert!(!l.contains("2026") && !l.ends_with('Z'), "{l}");
    }

    #[test]
    fn offset_labels() {
        assert_eq!(offset_label(0), "UTC");
        assert_eq!(offset_label(-6 * 3600), "UTC-06:00");
        assert_eq!(offset_label(5 * 3600 + 1800), "UTC+05:30");
    }
}
