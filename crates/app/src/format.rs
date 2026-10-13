//! Display formatting for sizes, counts, dates and durations.
//!
//! Pure functions only, so they can be unit-tested without a UI and cached per visible row.

use std::fmt::Write as _;
use std::time::Duration;

use chrono::{DateTime, Local, TimeZone};
use everythux_core::config::SizeFormat;

/// Formats an integer with `,` thousands separators: `1234567` becomes `1,234,567`.
pub(crate) fn format_count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Formats a byte count according to `format`.
///
/// Binary uses KiB/MiB/GiB, SI uses kB/MB/GB, and Bytes prints the exact count with separators.
pub(crate) fn format_size(bytes: u64, format: SizeFormat) -> String {
    let (base, units): (f64, &[&str]) = match format {
        SizeFormat::Bytes => return format!("{} B", format_count(bytes)),
        SizeFormat::Binary => (1024.0, &["KiB", "MiB", "GiB", "TiB", "PiB", "EiB"]),
        SizeFormat::Si => (1000.0, &["kB", "MB", "GB", "TB", "PB", "EB"]),
    };
    if (bytes as f64) < base {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / base;
    let mut unit = 0;
    while value >= base && unit + 1 < units.len() {
        value /= base;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", units[unit])
    } else {
        format!("{value:.1} {}", units[unit])
    }
}

/// Formats nanoseconds since the Unix epoch with a `chrono` strftime `format` in time zone `tz`.
///
/// An invalid format string falls back to ISO 8601 instead of panicking.
pub(crate) fn format_timestamp<Tz>(unix_ns: i64, format: &str, tz: &Tz) -> String
where
    Tz: TimeZone,
    Tz::Offset: std::fmt::Display,
{
    let secs = unix_ns.div_euclid(1_000_000_000);
    let nanos = unix_ns.rem_euclid(1_000_000_000) as u32;
    let Some(utc) = DateTime::from_timestamp(secs, nanos) else {
        return String::new();
    };
    let local = utc.with_timezone(tz);
    let mut out = String::with_capacity(16);
    if write!(out, "{}", local.format(format)).is_err() {
        out.clear();
        let _ = write!(out, "{}", local.format("%Y-%m-%d %H:%M"));
    }
    out
}

/// [`format_timestamp`] in the local time zone.
pub(crate) fn format_local(unix_ns: i64, format: &str) -> String {
    format_timestamp(unix_ns, format, &Local)
}

/// `12.3 s`, `450 ms` or `2 min 5 s`.
pub(crate) fn format_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 1.0 {
        format!("{} ms", d.as_millis())
    } else if secs < 60.0 {
        format!("{secs:.1} s")
    } else {
        let total = d.as_secs();
        format!("{} min {} s", total / 60, total % 60)
    }
}

/// `rwxr-xr-x` for the permission bits of `mode`.
pub(crate) fn format_permissions(mode: u32) -> String {
    const BITS: [(u32, char); 9] = [
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o040, 'r'),
        (0o020, 'w'),
        (0o010, 'x'),
        (0o004, 'r'),
        (0o002, 'w'),
        (0o001, 'x'),
    ];
    BITS.iter()
        .map(|&(bit, ch)| if mode & bit != 0 { ch } else { '-' })
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;

    #[test]
    fn counts_get_separators() {
        for (n, expected) in [
            (0, "0"),
            (12, "12"),
            (123, "123"),
            (1234, "1,234"),
            (1_234_567, "1,234,567"),
            (u64::MAX, "18,446,744,073,709,551,615"),
        ] {
            assert_eq!(format_count(n), expected, "{n}");
        }
    }

    #[test]
    fn sizes_binary_si_and_bytes() {
        for (bytes, fmt, expected) in [
            (0, SizeFormat::Binary, "0 B"),
            (1023, SizeFormat::Binary, "1023 B"),
            (1024, SizeFormat::Binary, "1.0 KiB"),
            (1536, SizeFormat::Binary, "1.5 KiB"),
            (150 * 1024 * 1024, SizeFormat::Binary, "150 MiB"),
            (1000, SizeFormat::Si, "1.0 kB"),
            (999, SizeFormat::Si, "999 B"),
            (2_500_000, SizeFormat::Si, "2.5 MB"),
            (1_234_567, SizeFormat::Bytes, "1,234,567 B"),
            (1 << 40, SizeFormat::Binary, "1.0 TiB"),
        ] {
            assert_eq!(format_size(bytes, fmt), expected, "{bytes} {fmt:?}");
        }
    }

    #[test]
    fn timestamps_use_the_configured_format() {
        let ns = 1_700_000_000_i64 * 1_000_000_000;
        assert_eq!(format_timestamp(ns, "%Y-%m-%d %H:%M", &Utc), "2023-11-14 22:13");
        assert_eq!(format_timestamp(ns, "%d.%m.%Y", &Utc), "14.11.2023");
    }

    #[test]
    fn pre_epoch_timestamps_do_not_panic() {
        assert_eq!(
            format_timestamp(-1_000_000_000, "%Y-%m-%d %H:%M:%S", &Utc),
            "1969-12-31 23:59:59"
        );
    }

    #[test]
    fn invalid_format_falls_back() {
        let ns = 1_700_000_000_i64 * 1_000_000_000;
        assert_eq!(format_timestamp(ns, "%Q%", &Utc), "2023-11-14 22:13");
    }

    #[test]
    fn durations_and_permissions() {
        assert_eq!(format_duration(Duration::from_millis(450)), "450 ms");
        assert_eq!(format_duration(Duration::from_millis(12_300)), "12.3 s");
        assert_eq!(format_duration(Duration::from_secs(125)), "2 min 5 s");
        assert_eq!(format_permissions(0o755), "rwxr-xr-x");
        assert_eq!(format_permissions(0o640), "rw-r-----");
        assert_eq!(format_permissions(0o100_644), "rw-r--r--");
    }
}
