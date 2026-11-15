//! Time parsing and the short human-readable forms the UI shows.

use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::Globalization::{GetDateFormatEx, GetTimeFormatEx, TIME_NOSECONDS};
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows::core::{PCWSTR, w};

/// Parses an RFC 3339 timestamp such as `2026-11-11T14:10:00.320228+00:00`
/// into unix seconds.
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let part = s.get(r)?;
        part.bytes().all(|c| c.is_ascii_digit()).then(|| part.parse().ok())?
    };
    if b.get(4) != Some(&b'-') || b.get(7) != Some(&b'-') || !matches!(b.get(10), Some(b'T' | b't' | b' ')) {
        return None;
    }
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, se) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let mut i = 19;
    if b.get(i) == Some(&b'.') {
        i += 1;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
    }
    let offset = match b.get(i) {
        Some(b'Z' | b'z') | None => 0,
        Some(&sign @ (b'+' | b'-')) => {
            let secs = num(i + 1..i + 3)? * 3600 + num(i + 4..i + 6)? * 60;
            if sign == b'-' { -secs } else { secs }
        }
        Some(_) => return None,
    };
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se - offset)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// A countdown like `2d 3h`, `4h 05m` or `12m` (rounded up to the minute).
pub fn countdown(secs: i64) -> String {
    let m = (secs.max(0) + 59) / 60;
    let (d, h, mm) = (m / 1440, (m % 1440) / 60, m % 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {mm:02}m")
    } else {
        format!("{mm}m")
    }
}

/// The local time's offset from UTC at any moment (see [`utc_offset`]),
/// asked once per half hour of time and remembered: for going through weeks
/// of events, across the days the clocks changed.
pub fn offsets() -> impl Fn(i64) -> i64 {
    let known = std::cell::RefCell::new(std::collections::HashMap::<i64, i64>::new());
    move |t| *known.borrow_mut().entry(t.div_euclid(1800)).or_insert_with(|| utc_offset(t))
}

/// The next time after `now` that the local clock shows `hour`:00 (unix seconds).
pub fn next_local(now: i64, hour: i64) -> i64 {
    let offset = utc_offset(now);
    let local = now + offset;
    let mut at = local - local.rem_euclid(86_400) + hour * 3600;
    if at <= local {
        at += 86_400;
    }
    // With the offset of that moment: the clocks may change before it.
    at - utc_offset(at - offset)
}

/// How long ago, compactly: `now`, `5m`, `2h`, `3d`.
pub fn ago(secs: i64) -> String {
    let s = secs.max(0);
    if s < 60 {
        crate::i18n::t("now").into()
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86_400)
    }
}

/// Offset of local time from UTC at `unix`, in seconds (e.g. 3600 in CET).
pub fn utc_offset(unix: i64) -> i64 {
    let ticks = (unix + 11_644_473_600).max(0) as u64 * 10_000_000;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    // SAFETY: pointers refer to live stack values.
    unsafe {
        if FileTimeToSystemTime(&ft, &mut utc).is_err()
            || SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_err()
        {
            return 0;
        }
    }
    let l = |v: u16| i64::from(v);
    let local_secs = days_from_civil(l(local.wYear), l(local.wMonth), l(local.wDay)) * 86_400
        + l(local.wHour) * 3600
        + l(local.wMinute) * 60
        + l(local.wSecond);
    local_secs - unix
}

/// Short local weekday name, e.g. `Thu`.
pub fn weekday(unix: i64) -> String {
    local_time(unix, true).split_whitespace().next().unwrap_or_default().to_string()
}

/// UTC time as RFC 3339, e.g. `2026-11-12T03:15:00Z`.
pub fn rfc3339(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", secs / 3600, secs % 3600 / 60, secs % 60)
}

/// The local date as `d MMM`, e.g. `11 Nov` (month names in the user's language).
pub fn local_date(unix: i64) -> String {
    let ticks = (unix + 11_644_473_600).max(0) as u64 * 10_000_000;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    let mut buf = [0u16; 64];
    // SAFETY: all pointers refer to live stack values; buffers are sized.
    unsafe {
        if FileTimeToSystemTime(&ft, &mut utc).is_err()
            || SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_err()
        {
            return String::new();
        }
        let n = GetDateFormatEx(
            crate::i18n::locale(),
            Default::default(),
            Some(&local),
            w!("d MMM"),
            Some(&mut buf),
            PCWSTR::null(),
        );
        if n > 1 { String::from_utf16_lossy(&buf[..n as usize - 1]) } else { String::new() }
    }
}

/// Local wall-clock time in the user's format, optionally with the weekday:
/// `14:10` or `Thu 14:10`.
pub fn local_time(unix: i64, with_day: bool) -> String {
    let ticks = (unix + 11_644_473_600).max(0) as u64 * 10_000_000;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    let mut buf = [0u16; 64];
    let mut out = String::new();
    // SAFETY: all pointers refer to live stack values; buffers are sized.
    unsafe {
        if FileTimeToSystemTime(&ft, &mut utc).is_err()
            || SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_err()
        {
            return out;
        }
        if with_day {
            let n = GetDateFormatEx(
                crate::i18n::locale(),
                Default::default(),
                Some(&local),
                w!("ddd"),
                Some(&mut buf),
                PCWSTR::null(),
            );
            if n > 1 {
                out = String::from_utf16_lossy(&buf[..n as usize - 1]) + " ";
            }
        }
        let n = GetTimeFormatEx(crate::i18n::locale(), TIME_NOSECONDS, Some(&local), PCWSTR::null(), Some(&mut buf));
        if n > 1 {
            out += &String::from_utf16_lossy(&buf[..n as usize - 1]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn finds_the_next_morning() {
        let now = 1_794_490_000;
        let at = super::next_local(now, 8);
        assert!(at > now && at <= now + 86_400);
        // 8:00 local time.
        assert_eq!((at + super::utc_offset(at)).rem_euclid(86_400), 8 * 3600);
    }

    use super::*;

    #[test]
    fn parses_rfc3339() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2026-11-11T14:10:00.320228+00:00"), Some(1_794_406_200));
        assert_eq!(parse_rfc3339("2026-11-11T16:10:00+02:00"), Some(1_794_406_200));
        assert_eq!(parse_rfc3339("2026-11-11T12:10:00-02:00"), Some(1_794_406_200));
        assert_eq!(parse_rfc3339("2024-02-29T00:00:00Z"), Some(1_709_164_800));
    }

    #[test]
    fn formats_rfc3339() {
        for t in [0, 1_794_406_200, 1_709_164_800, 951_782_400] {
            assert_eq!(parse_rfc3339(&rfc3339(t)), Some(t));
        }
        assert_eq!(rfc3339(1_794_406_200), "2026-11-11T14:10:00Z");
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_rfc3339(""), None);
        assert_eq!(parse_rfc3339("not a date"), None);
        assert_eq!(parse_rfc3339("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_rfc3339("2026-11-11T14:10:00X"), None);
        assert_eq!(parse_rfc3339("2026-1x-11T14:10:00Z"), None);
    }

    #[test]
    fn formats_countdown() {
        assert_eq!(countdown(-5), "0m");
        assert_eq!(countdown(1), "1m");
        assert_eq!(countdown(60), "1m");
        assert_eq!(countdown(61), "2m");
        assert_eq!(countdown(3600 + 5 * 60), "1h 05m");
        assert_eq!(countdown(2 * 86_400 + 3 * 3600), "2d 3h");
    }

    #[test]
    fn formats_ago() {
        assert_eq!(ago(10), "now");
        assert_eq!(ago(125), "2m");
        assert_eq!(ago(7200), "2h");
        assert_eq!(ago(3 * 86_400), "3d");
    }
}
