//! Epoch arithmetic, ISO 8601 parsing and local-time reset labels.

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn num(b: &[u8]) -> Option<i64> {
    if b.is_empty() || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(b.iter().fold(0i64, |n, c| n * 10 + i64::from(c - b'0')))
}

/// Parse `YYYY-MM-DDTHH:MM:SS[.frac][Z|±HH:MM]` into epoch seconds.
/// A missing offset is read as UTC, which is what cswap and the API emit.
pub fn parse_iso(s: &str) -> Option<f64> {
    let b = s.trim().as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    if b[10] != b'T' && b[10] != b't' && b[10] != b' ' {
        return None;
    }
    let (y, mo, d) = (num(&b[0..4])?, num(&b[5..7])?, num(&b[8..10])?);
    let (h, mi, sec) = (num(&b[11..13])?, num(&b[14..16])?, num(&b[17..19])?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let mut i = 19;
    let mut frac = 0.0;
    if i < b.len() && b[i] == b'.' {
        let start = i + 1;
        i = start;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        let digits = &b[start..i];
        if let Some(n) = num(digits) {
            frac = n as f64 / 10f64.powi(digits.len() as i32);
        }
    }
    let offset = match b.get(i) {
        None | Some(b'Z' | b'z') => 0,
        Some(&sign @ (b'+' | b'-')) => {
            let rest = &b[i + 1..];
            let (oh, om) = match rest.len() {
                5 if rest[2] == b':' => (num(&rest[0..2])?, num(&rest[3..5])?),
                4 => (num(&rest[0..2])?, num(&rest[2..4])?),
                2 => (num(rest)?, 0),
                _ => return None,
            };
            let o = oh * 3600 + om * 60;
            if sign == b'-' {
                -o
            } else {
                o
            }
        }
        _ => return None,
    };
    let secs = days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + sec - offset;
    Some(secs as f64 + frac)
}

/// Weekday (0 = Sunday), hour and minute of an epoch in the local zone.
#[cfg(unix)]
fn local_wday_hm(epoch: i64) -> (i64, i64, i64) {
    extern "C" {
        fn tzset();
    }
    // SAFETY: tzset has no preconditions; localtime_r writes only into `tm`.
    unsafe {
        tzset();
        let t = epoch as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return utc_wday_hm(epoch);
        }
        (i64::from(tm.tm_wday), i64::from(tm.tm_hour), i64::from(tm.tm_min))
    }
}

#[cfg(windows)]
fn local_wday_hm(epoch: i64) -> (i64, i64, i64) {
    #[repr(C)]
    struct Tm {
        sec: i32,
        min: i32,
        hour: i32,
        mday: i32,
        mon: i32,
        year: i32,
        wday: i32,
        yday: i32,
        isdst: i32,
    }
    extern "C" {
        fn _tzset();
        fn _localtime64_s(tm: *mut Tm, t: *const i64) -> i32;
    }
    let mut tm = Tm {
        sec: 0,
        min: 0,
        hour: 0,
        mday: 0,
        mon: 0,
        year: 0,
        wday: 0,
        yday: 0,
        isdst: 0,
    };
    // SAFETY: both CRT calls only read `epoch` and write into `tm`.
    let rc = unsafe {
        _tzset();
        _localtime64_s(&mut tm, &epoch)
    };
    if rc != 0 {
        return utc_wday_hm(epoch);
    }
    (i64::from(tm.wday), i64::from(tm.hour), i64::from(tm.min))
}

#[cfg(not(any(unix, windows)))]
fn local_wday_hm(epoch: i64) -> (i64, i64, i64) {
    utc_wday_hm(epoch)
}

fn utc_wday_hm(epoch: i64) -> (i64, i64, i64) {
    let days = epoch.div_euclid(86_400);
    let secs = epoch.rem_euclid(86_400);
    ((days + 4).rem_euclid(7), secs / 3600, secs % 3600 / 60)
}

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// Time left until `reset`: `45m`, `2h36m`, then `Sun 2AM` (local) past a day.
/// Minutes round up, so a window that has not reset never reads `0m`.
pub fn reset_label(reset: i64, now: i64) -> String {
    let diff = reset - now;
    if diff <= 0 {
        return "now".to_string();
    }
    let mins = (diff + 59) / 60;
    if mins < 60 {
        return format!("{mins}m");
    }
    let (h, m) = (mins / 60, mins % 60);
    if h < 24 {
        return format!("{h}h{m}m");
    }
    // Round to the minute so 05:59:59.9 reads as 6AM, not 5AM.
    let (wday, hour, minute) = local_wday_hm((reset + 30).div_euclid(60) * 60);
    let h12 = if hour % 12 == 0 { 12 } else { hour % 12 };
    let ampm = if hour < 12 { "AM" } else { "PM" };
    let day = WEEKDAYS[wday.rem_euclid(7) as usize];
    if minute == 0 {
        format!("{day} {h12}{ampm}")
    } else {
        format!("{day} {h12}:{minute:02}{ampm}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_variants() {
        assert_eq!(parse_iso("2026-11-15T08:00:00Z"), Some(1_794_729_600.0));
        assert_eq!(parse_iso("2026-11-15T08:00:00+00:00"), Some(1_794_729_600.0));
        assert_eq!(parse_iso("2026-11-15T02:00:00-06:00"), Some(1_794_729_600.0));
        assert_eq!(parse_iso("2026-11-15T08:00:00"), Some(1_794_729_600.0));
        let f = parse_iso("2026-11-15T08:00:00.505763+00:00").unwrap();
        assert!((f - 1_794_729_600.505763).abs() < 1e-6);
        assert_eq!(parse_iso("1970-01-01T00:00:00Z"), Some(0.0));
        assert_eq!(parse_iso("2024-02-29T12:00:00Z"), Some(1_709_208_000.0));
        assert_eq!(parse_iso("garbage"), None);
        assert_eq!(parse_iso("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_iso("2026-11-15T08:00:00+0"), None);
    }

    #[test]
    fn utc_weekday() {
        // 2026-11-15 is a Sunday.
        assert_eq!(utc_wday_hm(1_794_729_600), (0, 8, 0));
        assert_eq!(utc_wday_hm(0), (4, 0, 0)); // Thursday
        assert_eq!(utc_wday_hm(-1), (3, 23, 59));
    }

    #[test]
    fn short_labels() {
        let now = 1_000_000;
        assert_eq!(reset_label(now, now), "now");
        assert_eq!(reset_label(now - 5, now), "now");
        assert_eq!(reset_label(now + 1, now), "1m");
        assert_eq!(reset_label(now + 60, now), "1m");
        assert_eq!(reset_label(now + 61, now), "2m");
        assert_eq!(reset_label(now + 3600, now), "1h0m");
        assert_eq!(reset_label(now + 2 * 3600 + 36 * 60, now), "2h36m");
        assert_eq!(reset_label(now + 24 * 3600 - 60, now), "23h59m");
    }
}
