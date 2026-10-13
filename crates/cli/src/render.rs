//! Pure text rendering: sizes, dates, tables and the text of each report. Nothing here does I/O,
//! so everything is unit-tested; printing lives in [`crate::output`].

use std::fmt::{Display, Write as _};
use std::path::Path;

use chrono::{DateTime, SecondsFormat, TimeZone};
use everythux_core::exclusion::{ExclusionRule, RuleKind};
use everythux_core::model::EntryKind;
use everythux_index::{DiscoveredVolume, JobStats, Progress, StoreStats};

use crate::commands::volumes::VolumeRow;

/// `1536` becomes `1.5 KiB` (binary units, one decimal).
pub(crate) fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    // Rounding to one decimal can produce "1024.0 KiB"; move up a unit instead.
    while (value * 10.0).round() / 10.0 >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// `1234567` becomes `1,234,567`.
pub(crate) fn human_count(n: u64) -> String {
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

/// Formats nanoseconds since the Unix epoch as `YYYY-MM-DD HH:MM` in `tz`.
pub(crate) fn format_datetime<Tz>(ns: i64, tz: &Tz) -> String
where
    Tz: TimeZone,
    Tz::Offset: Display,
{
    DateTime::from_timestamp_nanos(ns)
        .with_timezone(tz)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

/// RFC 3339 in UTC with second precision (`2026-01-02T03:04:05Z`), as used by `search --json`.
pub(crate) fn rfc3339(ns: i64) -> String {
    DateTime::from_timestamp_nanos(ns).to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Name of an entry kind in JSON output.
pub(crate) const fn kind_name(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::File => "file",
        EntryKind::Dir => "dir",
        EntryKind::Symlink => "symlink",
        EntryKind::Other => "other",
    }
}

/// Name of a rule kind as typed on the command line.
pub(crate) const fn rule_kind_name(kind: RuleKind) -> &'static str {
    match kind {
        RuleKind::Dir => "dir",
        RuleKind::File => "file",
        RuleKind::Path => "path",
        RuleKind::Any => "any",
    }
}

/// Left-aligned columns separated by two spaces; the last column isn't padded.
pub(crate) fn render_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.chars().count());
        }
    }
    let mut out = String::new();
    let mut line = |cells: &mut dyn Iterator<Item = &str>| {
        let cells: Vec<&str> = cells.collect();
        for (i, cell) in cells.iter().enumerate() {
            if i + 1 == cells.len() {
                out.push_str(cell);
            } else {
                let _ = write!(out, "{cell:<width$}  ", width = widths[i]);
            }
        }
        out.push('\n');
    };
    line(&mut headers.iter().copied());
    for row in rows {
        line(&mut row.iter().map(String::as_str));
    }
    out
}

/// The `volumes` report.
pub(crate) fn render_volumes(rows: &[VolumeRow]) -> String {
    if rows.is_empty() {
        return "No volumes known or mounted.\n".to_owned();
    }
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            vec![
                r.state.to_owned(),
                r.mount_point.display().to_string(),
                r.uuid.clone(),
                r.label.clone().unwrap_or_default(),
                r.fs_type.clone(),
                r.last_scan_ns
                    .map_or_else(|| "never".to_owned(), |ns| format_datetime(ns, &chrono::Local)),
            ]
        })
        .collect();
    render_table(
        &["STATE", "MOUNT POINT", "UUID", "LABEL", "FS", "LAST FULL SCAN"],
        &cells,
    )
}

/// The `scan --dry-run` report.
pub(crate) fn render_discovered(
    volumes: &[DiscoveredVolume],
    active_rules: usize,
    total_rules: usize,
) -> String {
    let mut out = if volumes.is_empty() {
        "No volumes would be indexed.\n".to_owned()
    } else {
        let cells: Vec<Vec<String>> = volumes
            .iter()
            .map(|v| {
                vec![
                    v.mount_point.display().to_string(),
                    v.device.display().to_string(),
                    v.fs_type.clone(),
                    v.fs_uuid.clone(),
                    v.label.clone().unwrap_or_default(),
                ]
            })
            .collect();
        render_table(&["MOUNT POINT", "DEVICE", "FS", "UUID", "LABEL"], &cells)
    };
    let _ = writeln!(
        out,
        "\n{active_rules} of {total_rules} exclusion rules are active. Dry run: nothing was scanned."
    );
    out
}

/// The `stats` report.
pub(crate) fn render_stats(stats: &StoreStats) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Entries:  {}", human_count(stats.entries));
    let _ = writeln!(out, "Folders:  {}", human_count(stats.dirs));
    let per_entry = stats
        .db_bytes
        .checked_div(stats.entries)
        .map_or_else(String::new, |bytes| format!(" ({bytes} B/entry)"));
    let _ = writeln!(out, "Database: {}{per_entry}", human_size(stats.db_bytes));
    if !stats.volumes.is_empty() {
        out.push('\n');
        let cells: Vec<Vec<String>> = stats
            .volumes
            .iter()
            .map(|(v, entries)| {
                vec![
                    v.state.as_str().to_owned(),
                    v.mount_point.display().to_string(),
                    human_count(*entries),
                    v.last_full_scan_ns
                        .map_or_else(|| "never".to_owned(), |ns| format_datetime(ns, &chrono::Local)),
                    v.last_scan_ms
                        .map_or_else(String::new, |ms| format!("{:.1}s", ms as f64 / 1000.0)),
                ]
            })
            .collect();
        out.push_str(&render_table(
            &["STATE", "MOUNT POINT", "ENTRIES", "LAST FULL SCAN", "TOOK"],
            &cells,
        ));
    }
    out
}

/// One-line description of a rule, e.g. `dir  bin  (under /home/*)`.
pub(crate) fn describe_rule(rule: &ExclusionRule) -> String {
    let scope = rule
        .scope
        .as_deref()
        .map_or_else(String::new, |s| format!("  (under {s})"));
    format!("{}  {}{scope}", rule_kind_name(rule.kind), rule.pattern)
}

/// The `exclude list` report; indexes are the ones other `exclude` subcommands take.
pub(crate) fn render_exclusions(rules: &[ExclusionRule]) -> String {
    if rules.is_empty() {
        return "No exclusion rules.\n".to_owned();
    }
    let cells: Vec<Vec<String>> = rules
        .iter()
        .enumerate()
        .map(|(i, r)| {
            vec![
                i.to_string(),
                if r.enabled { "on" } else { "off" }.to_owned(),
                rule_kind_name(r.kind).to_owned(),
                r.pattern.clone(),
                r.scope.clone().unwrap_or_default(),
            ]
        })
        .collect();
    render_table(&["#", "STATE", "KIND", "PATTERN", "SCOPE"], &cells)
}

/// The `exclude test` answer.
pub(crate) fn render_verdict(
    path: &Path,
    is_dir: bool,
    assumed_dir: bool,
    matched: Option<(usize, &ExclusionRule)>,
) -> String {
    let what = match (is_dir, assumed_dir) {
        (true, true) => "folder (assumed, the path doesn't exist)",
        (true, false) => "folder",
        (false, _) => "file",
    };
    match matched {
        Some((index, rule)) => format!(
            "{} ({what}) is excluded by rule #{index}: {}\n",
            path.display(),
            describe_rule(rule)
        ),
        None => format!("{} ({what}) is not excluded.\n", path.display()),
    }
}

/// Single-line progress text for the running job.
pub(crate) fn render_progress(p: &Progress, max_width: usize) -> String {
    let mut line = String::new();
    if let Some(volume) = &p.volume {
        let _ = write!(line, "{}  ", volume.display());
    }
    let _ = write!(
        line,
        "{} folders, {} entries",
        human_count(p.dirs),
        human_count(p.entries)
    );
    if p.errors > 0 {
        let _ = write!(line, ", {} errors", human_count(p.errors));
    }
    if let Some(fraction) = p.fraction {
        let _ = write!(line, ", ~{:.0}%", (fraction.clamp(0.0, 1.0) * 100.0));
    }
    if !p.current.as_os_str().is_empty() {
        let _ = write!(line, "  {}", p.current.display());
    }
    truncate_chars(&line, max_width)
}

/// Shortens `s` to at most `max` characters, ending in `…` when cut.
pub(crate) fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let keep = max.saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    out.push('…');
    out
}

/// Summary line after a job.
pub(crate) fn render_job_stats(stats: &JobStats) -> String {
    format!(
        "Done in {:.1}s: {} entries added or updated, {} removed, {} folders listed, {} errors.\n",
        stats.elapsed.as_secs_f64(),
        human_count(stats.upserted),
        human_count(stats.removed),
        human_count(stats.dirs_listed),
        human_count(stats.errors),
    )
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use everythux_core::model::{VolumeInfo, VolumeState};

    use super::*;

    #[test]
    fn sizes_use_binary_units() {
        for (bytes, expected) in [
            (0, "0 B"),
            (1023, "1023 B"),
            (1024, "1.0 KiB"),
            (1536, "1.5 KiB"),
            (1_048_575, "1.0 MiB"),
            (150 * 1024 * 1024, "150.0 MiB"),
            (u64::MAX, "16.0 EiB"),
        ] {
            assert_eq!(human_size(bytes), expected, "{bytes}");
        }
    }

    #[test]
    fn counts_get_thousands_separators() {
        for (n, expected) in [(0, "0"), (999, "999"), (1000, "1,000"), (1_234_567, "1,234,567")] {
            assert_eq!(human_count(n), expected);
        }
    }

    #[test]
    fn dates_are_rendered_in_the_given_zone() {
        let ns = 1_767_323_045_000_000_000; // 2026-01-02T03:04:05Z
        assert_eq!(format_datetime(ns, &Utc), "2026-01-02 03:04");
        assert_eq!(rfc3339(ns), "2026-01-02T03:04:05Z");
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(-1_000_000_000), "1969-12-31T23:59:59Z");
    }

    #[test]
    fn tables_align_columns_and_trim_the_last() {
        let rows = vec![
            vec!["1".to_owned(), "alpha".to_owned()],
            vec!["22".to_owned(), "b".to_owned()],
        ];
        assert_eq!(
            render_table(&["#", "NAME"], &rows),
            "#   NAME\n1   alpha\n22  b\n"
        );
    }

    #[test]
    fn truncation_counts_characters() {
        assert_eq!(truncate_chars("abc", 5), "abc");
        assert_eq!(truncate_chars("abcdef", 4), "abc…");
        assert_eq!(truncate_chars("äöüäöü", 3), "äö…");
    }

    #[test]
    fn progress_line_mentions_volume_counts_and_percent() {
        let p = Progress {
            volume: Some("/home".into()),
            dirs: 1200,
            entries: 34_567,
            errors: 2,
            current: "/home/me/src".into(),
            fraction: Some(0.425),
        };
        let line = render_progress(&p, 200);
        assert_eq!(
            line,
            "/home  1,200 folders, 34,567 entries, 2 errors, ~42%  /home/me/src"
        );
        assert_eq!(render_progress(&p, 20).chars().count(), 20);
    }

    #[test]
    fn exclusion_list_and_verdicts() {
        let rules = vec![
            ExclusionRule::new("bin", RuleKind::Dir).scoped("/home/*"),
            ExclusionRule::new("*.tmp", RuleKind::File),
        ];
        let list = render_exclusions(&rules);
        assert!(list.contains("0  on     dir   bin      /home/*"), "{list}");
        assert!(list.contains("1  on     file  *.tmp"), "{list}");
        assert_eq!(describe_rule(&rules[0]), "dir  bin  (under /home/*)");
        let v = render_verdict(Path::new("/home/a/bin"), true, false, Some((0, &rules[0])));
        assert_eq!(
            v,
            "/home/a/bin (folder) is excluded by rule #0: dir  bin  (under /home/*)\n"
        );
        let v = render_verdict(Path::new("/usr/bin"), true, true, None);
        assert!(v.contains("assumed") && v.contains("not excluded"), "{v}");
    }

    #[test]
    fn stats_report_includes_bytes_per_entry() {
        let stats = StoreStats {
            entries: 2000,
            dirs: 100,
            db_bytes: 300_000,
            volumes: vec![(
                VolumeInfo {
                    fs_uuid: "u".into(),
                    device: "/dev/sda1".into(),
                    mount_point: "/".into(),
                    label: None,
                    fs_type: "ext4".into(),
                    state: VolumeState::Online,
                    last_full_scan_ns: None,
                    last_scan_ms: Some(1500),
                },
                2000,
            )],
        };
        let text = render_stats(&stats);
        assert!(text.contains("Entries:  2,000"), "{text}");
        assert!(text.contains("(150 B/entry)"), "{text}");
        let row = text
            .lines()
            .find(|l| l.starts_with("online"))
            .expect("volume row");
        assert!(
            row.contains(" /  ") && row.contains("2,000") && row.contains("never"),
            "{text}"
        );
        assert!(text.contains("1.5s"), "{text}");
    }
}
