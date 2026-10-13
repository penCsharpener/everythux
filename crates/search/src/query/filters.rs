//! Value parsers for the `ext:`, `size:` and `dm:` filters.

use chrono::{Datelike, Days, Months, NaiveDate};

/// Inclusive size bounds in bytes. `None` when the range is empty.
pub(super) fn parse_size(value: &str) -> Result<Option<(u64, u64)>, String> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("empty") {
        return Ok(Some((0, 0)));
    }
    if let Some((lo, hi)) = value.split_once("..") {
        let lo = if lo.is_empty() { 0 } else { size_number(lo)? };
        let hi = if hi.is_empty() { u64::MAX } else { size_number(hi)? };
        return Ok((lo <= hi).then_some((lo, hi)));
    }
    for (prefix, op) in [
        (">=", Cmp::Ge),
        ("<=", Cmp::Le),
        (">", Cmp::Gt),
        ("<", Cmp::Lt),
        ("=", Cmp::Eq),
    ] {
        if let Some(rest) = value.strip_prefix(prefix) {
            let n = size_number(rest)?;
            return Ok(match op {
                Cmp::Ge => Some((n, u64::MAX)),
                Cmp::Gt => n.checked_add(1).map(|lo| (lo, u64::MAX)),
                Cmp::Le => Some((0, n)),
                Cmp::Lt => n.checked_sub(1).map(|hi| (0, hi)),
                Cmp::Eq => Some((n, n)),
            });
        }
    }
    let n = size_number(value)?;
    Ok(Some((n, n)))
}

#[derive(Clone, Copy)]
enum Cmp {
    Ge,
    Gt,
    Le,
    Lt,
    Eq,
}

/// `10`, `1.5mb`, `100KiB`. All multi-byte units are powers of 1024.
fn size_number(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let split = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let (num, unit) = text.split_at(split);
    if num.is_empty() {
        return Err(format!("expected a number in {text:?}"));
    }
    let value: f64 = num.parse().map_err(|_| format!("invalid number {num:?}"))?;
    let shift = match unit.to_ascii_lowercase().as_str() {
        "" | "b" => 0,
        "k" | "kb" | "kib" => 1,
        "m" | "mb" | "mib" => 2,
        "g" | "gb" | "gib" => 3,
        "t" | "tb" | "tib" => 4,
        "p" | "pb" | "pib" => 5,
        other => return Err(format!("unknown size unit {other:?}")),
    };
    let bytes = value * 1024f64.powi(shift);
    // float -> int casts saturate, which is what we want for absurdly large inputs
    Ok(bytes as u64)
}

/// The local calendar, injected so tests don't depend on the wall clock or timezone.
pub(super) struct Calendar<'a> {
    /// Today's date in the local timezone.
    pub today: NaiveDate,
    /// Local midnight of a date, as ns since the Unix epoch.
    pub midnight_ns: &'a dyn Fn(NaiveDate) -> i64,
}

/// `[start, end)` of a modification-time filter in ns. `None` when the range is empty.
pub(super) fn parse_date(value: &str, cal: &Calendar<'_>) -> Result<Option<(i64, i64)>, String> {
    let value = value.trim();
    let range = if let Some((lo, hi)) = value.split_once("..") {
        let lo = if lo.is_empty() {
            i64::MIN
        } else {
            period(lo, cal)?.0
        };
        let hi = if hi.is_empty() {
            i64::MAX
        } else {
            period(hi, cal)?.1
        };
        (lo, hi)
    } else if let Some(rest) = value.strip_prefix(">=") {
        (period(rest, cal)?.0, i64::MAX)
    } else if let Some(rest) = value.strip_prefix('>') {
        (period(rest, cal)?.1, i64::MAX)
    } else if let Some(rest) = value.strip_prefix("<=") {
        (i64::MIN, period(rest, cal)?.1)
    } else if let Some(rest) = value.strip_prefix('<') {
        (i64::MIN, period(rest, cal)?.0)
    } else {
        period(value.strip_prefix('=').unwrap_or(value), cal)?
    };
    Ok((range.0 < range.1).then_some(range))
}

/// A keyword or date as a half-open interval of ns.
fn period(text: &str, cal: &Calendar<'_>) -> Result<(i64, i64), String> {
    let text = text.trim();
    let (start, end) = date_period(text, cal.today).ok_or_else(|| format!("unrecognised date {text:?}"))?;
    Ok(((cal.midnight_ns)(start), (cal.midnight_ns)(end)))
}

/// `[start, end)` as dates (end exclusive).
fn date_period(text: &str, today: NaiveDate) -> Option<(NaiveDate, NaiveDate)> {
    let tomorrow = today.checked_add_days(Days::new(1))?;
    let month_start = today.with_day(1)?;
    let year_start = today.with_day(1)?.with_month(1)?;
    let week_start = today.checked_sub_days(Days::new(u64::from(today.weekday().num_days_from_monday())))?;
    let next = |d: NaiveDate, months: u32| d.checked_add_months(Months::new(months));
    match text.to_ascii_lowercase().as_str() {
        "today" => return Some((today, tomorrow)),
        "yesterday" => return Some((today.pred_opt()?, today)),
        "thisweek" => return Some((week_start, week_start.checked_add_days(Days::new(7))?)),
        "lastweek" => return Some((week_start.checked_sub_days(Days::new(7))?, week_start)),
        "thismonth" => return Some((month_start, next(month_start, 1)?)),
        "lastmonth" => return Some((month_start.checked_sub_months(Months::new(1))?, month_start)),
        "thisyear" => return Some((year_start, next(year_start, 12)?)),
        "lastyear" => return Some((year_start.checked_sub_months(Months::new(12))?, year_start)),
        _ => {}
    }
    let mut parts = text.split(['-', '/', '.']);
    let year: i32 = parse_digits(parts.next()?, 4, 4)?.try_into().ok()?;
    let month = match parts.next() {
        Some(m) => Some(parse_digits(m, 1, 2)?),
        None => None,
    };
    let day = match parts.next() {
        Some(d) => Some(parse_digits(d, 1, 2)?),
        None => None,
    };
    if parts.next().is_some() {
        return None;
    }
    match (month, day) {
        (None, _) => {
            let start = NaiveDate::from_ymd_opt(year, 1, 1)?;
            Some((start, next(start, 12)?))
        }
        (Some(m), None) => {
            let start = NaiveDate::from_ymd_opt(year, m, 1)?;
            Some((start, next(start, 1)?))
        }
        (Some(m), Some(d)) => {
            let start = NaiveDate::from_ymd_opt(year, m, d)?;
            Some((start, start.checked_add_days(Days::new(1))?))
        }
    }
}

fn parse_digits(s: &str, min: usize, max: usize) -> Option<u32> {
    (s.len() >= min && s.len() <= max && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().ok())
        .flatten()
}

/// `pdf;docx` -> `["pdf", "docx"]`: lowercase, without leading dots, deduplicated.
pub(super) fn parse_ext(value: &str) -> Vec<Box<str>> {
    let mut out: Vec<Box<str>> = Vec::new();
    for part in value.split([';', ',']) {
        let ext = part.trim().trim_start_matches('.').to_lowercase();
        if !ext.is_empty() && !out.iter().any(|e| **e == *ext) {
            out.push(ext.into_boxed_str());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const KB: u64 = 1024;

    #[test]
    fn size_forms() {
        for (input, expected) in [
            (">10mb", Some((10 * KB * KB + 1, u64::MAX))),
            (">=10MB", Some((10 * KB * KB, u64::MAX))),
            ("<1kb", Some((0, KB - 1))),
            ("<=1KiB", Some((0, KB))),
            ("=5", Some((5, 5))),
            ("5", Some((5, 5))),
            ("1mb..10mb", Some((KB * KB, 10 * KB * KB))),
            ("..1gb", Some((0, KB * KB * KB))),
            ("1.5kb..", Some((1536, u64::MAX))),
            ("2tb", Some((2 * KB * KB * KB * KB, 2 * KB * KB * KB * KB))),
            ("empty", Some((0, 0))),
            ("<0", None),
            ("10mb..1mb", None),
        ] {
            assert_eq!(parse_size(input), Ok(expected), "{input}");
        }
    }

    #[test]
    fn size_errors() {
        for input in [">", "abc", "10xb", ">=mb", "1..x"] {
            assert!(parse_size(input).is_err(), "{input}");
        }
    }

    fn cal(f: impl FnOnce(&Calendar<'_>)) {
        // Pretend the timezone is UTC: midnight ns = days * 86400 * 1e9.
        let midnight =
            |d: NaiveDate| d.and_time(chrono::NaiveTime::MIN).and_utc().timestamp() * 1_000_000_000;
        let today = NaiveDate::from_ymd_opt(2026, 3, 18).unwrap(); // a Wednesday
        f(&Calendar {
            today,
            midnight_ns: &midnight,
        });
    }

    fn range_dates(value: &str, c: &Calendar<'_>) -> Option<(String, String)> {
        let (lo, hi) = parse_date(value, c).unwrap()?;
        let show = |ns: i64| {
            if ns == i64::MIN {
                "-inf".to_owned()
            } else if ns == i64::MAX {
                "+inf".to_owned()
            } else {
                chrono::DateTime::from_timestamp(ns / 1_000_000_000, 0)
                    .unwrap()
                    .format("%Y-%m-%d")
                    .to_string()
            }
        };
        Some((show(lo), show(hi)))
    }

    #[test]
    fn date_keywords_and_literals() {
        cal(|c| {
            for (input, from, to) in [
                ("today", "2026-03-18", "2026-03-19"),
                ("yesterday", "2026-03-17", "2026-03-18"),
                ("thisweek", "2026-03-16", "2026-03-23"),
                ("lastweek", "2026-03-09", "2026-03-16"),
                ("thismonth", "2026-03-01", "2026-04-01"),
                ("lastmonth", "2026-02-01", "2026-03-01"),
                ("thisyear", "2026-01-01", "2027-01-01"),
                ("lastyear", "2025-01-01", "2026-01-01"),
                ("2026-01-05", "2026-01-05", "2026-01-06"),
                ("2025-12", "2025-12-01", "2026-01-01"),
                ("2024", "2024-01-01", "2025-01-01"),
                ("2026-01-01..2026-03-01", "2026-01-01", "2026-03-02"),
                ("2025..2026-02", "2025-01-01", "2026-03-01"),
                ("lastweek..today", "2026-03-09", "2026-03-19"),
                (">2026-03-01", "2026-03-02", "+inf"),
                (">=2026-03-01", "2026-03-01", "+inf"),
                ("<2026-03-01", "-inf", "2026-03-01"),
                ("<=2026-03", "-inf", "2026-04-01"),
                ("..2020", "-inf", "2021-01-01"),
                ("2020..", "2020-01-01", "+inf"),
            ] {
                assert_eq!(
                    range_dates(input, c),
                    Some((from.to_owned(), to.to_owned())),
                    "{input}"
                );
            }
        });
    }

    #[test]
    fn date_errors_and_empty_ranges() {
        cal(|c| {
            for input in [
                "tomorrowish",
                "2026-13-01",
                "2026-02-30",
                "26",
                "2026-1-2-3",
                "x..y",
            ] {
                assert!(parse_date(input, c).is_err(), "{input}");
            }
            assert_eq!(parse_date("2026-03-01..2025-01-01", c), Ok(None));
        });
    }

    #[test]
    fn ext_lists() {
        let exts = |s| parse_ext(s).iter().map(ToString::to_string).collect::<Vec<_>>();
        assert_eq!(exts("pdf;DOCX"), ["pdf", "docx"]);
        assert_eq!(exts(".jpg;;jpg, png"), ["jpg", "png"]);
        assert!(exts(";").is_empty());
    }
}
