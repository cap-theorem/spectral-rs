use std::time::{Duration, SystemTime, UNIX_EPOCH};

const NANOS_PER_SEC: u128 = 1_000_000_000;
const NANOS_PER_MILLI: u128 = 1_000_000;

pub(crate) fn format_duration(d: Duration) -> String {
    let nanos = d.as_nanos();
    if nanos.is_multiple_of(NANOS_PER_SEC) {
        format!("{}s", nanos / NANOS_PER_SEC)
    } else if nanos.is_multiple_of(NANOS_PER_MILLI) {
        format!("{}ms", nanos / NANOS_PER_MILLI)
    } else {
        format!("{nanos}ns")
    }
}

pub(crate) fn parse_duration(text: &str) -> Option<Duration> {
    let rest = text.trim().strip_prefix('+').unwrap_or(text.trim());
    if rest == "0" {
        return Some(Duration::ZERO);
    }
    if rest.is_empty() || rest.starts_with('-') {
        return None;
    }

    let mut total = 0f64;
    let mut rest = rest;
    while !rest.is_empty() {
        let number_len = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        if number_len == 0 {
            return None;
        }
        let amount: f64 = rest[..number_len].parse().ok()?;
        rest = &rest[number_len..];

        let (unit_secs, unit_len) = [
            ("ns", 1e-9),
            ("us", 1e-6),
            ("µs", 1e-6),
            ("μs", 1e-6),
            ("ms", 1e-3),
            ("s", 1.0),
            ("m", 60.0),
            ("h", 3600.0),
        ]
        .iter()
        .find(|(unit, _)| rest.starts_with(unit))
        .map(|(unit, secs)| (*secs, unit.len()))?;

        total += amount * unit_secs;
        rest = &rest[unit_len..];
    }

    Duration::try_from_secs_f64(total).ok()
}

pub(crate) fn parse_rfc3339(text: &str) -> Option<SystemTime> {
    let b = text.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || !matches!(b[10], b'T' | b't')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }

    let year: i64 = digits(&b[0..4])?;
    let month: i64 = digits(&b[5..7])?;
    let day: i64 = digits(&b[8..10])?;
    let hour: i64 = digits(&b[11..13])?;
    let minute: i64 = digits(&b[14..16])?;
    let second: i64 = digits(&b[17..19])?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }

    let mut pos = 19;
    let mut nanos: u32 = 0;
    if b[pos] == b'.' {
        let start = pos + 1;
        let mut end = start;
        while end < b.len() && b[end].is_ascii_digit() {
            end += 1;
        }
        if end == start {
            return None;
        }
        let mut scaled = [b'0'; 9];
        let take = (end - start).min(9);
        scaled[..take].copy_from_slice(&b[start..start + take]);
        nanos = digits(&scaled)? as u32;
        pos = end;
    }

    let offset_secs = match &b[pos..] {
        [b'Z' | b'z'] => 0,
        [sign @ (b'+' | b'-'), h1, h2, b':', m1, m2] => {
            let hours: i64 = digits(&[*h1, *h2])?;
            let minutes: i64 = digits(&[*m1, *m2])?;
            let offset = hours * 3600 + minutes * 60;
            if *sign == b'-' { -offset } else { offset }
        }
        _ => return None,
    };

    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + hour * 3600 + minute * 60 + second - offset_secs;
    if secs >= 0 {
        UNIX_EPOCH.checked_add(Duration::new(secs as u64, nanos))
    } else {
        UNIX_EPOCH
            .checked_sub(Duration::from_secs(secs.unsigned_abs()))?
            .checked_add(Duration::from_nanos(u64::from(nanos)))
    }
}

fn digits(b: &[u8]) -> Option<i64> {
    let mut value: i64 = 0;
    for &c in b {
        if !c.is_ascii_digit() {
            return None;
        }
        value = value * 10 + i64::from(c - b'0');
    }
    Some(value)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_durations() {
        assert_eq!(format_duration(Duration::from_secs(10)), "10s");
        assert_eq!(format_duration(Duration::from_millis(1500)), "1500ms");
        assert_eq!(format_duration(Duration::from_nanos(1_500)), "1500ns");
    }

    #[test]
    fn parses_durations() {
        let cases = [
            ("10s", Duration::from_secs(10)),
            ("1m30s", Duration::from_secs(90)),
            ("1.5s", Duration::from_millis(1500)),
            ("250ms", Duration::from_millis(250)),
            ("1h", Duration::from_secs(3600)),
            ("0s", Duration::ZERO),
            ("0", Duration::ZERO),
            ("500µs", Duration::from_micros(500)),
        ];
        for (text, want) in cases {
            let got = parse_duration(text).unwrap_or_else(|| panic!("{text} did not parse"));
            assert!(
                got.abs_diff(want) < Duration::from_nanos(10),
                "{text}: {got:?} != {want:?}"
            );
        }
    }

    #[test]
    fn rejects_bad_durations() {
        for text in ["", "10", "10x", "s", "1s2", "-2s"] {
            assert_eq!(parse_duration(text), None, "{text} should not parse");
        }
    }

    #[test]
    fn parses_rfc3339() {
        let utc = parse_rfc3339("2026-11-16T20:00:10Z").unwrap();
        assert_eq!(
            utc.duration_since(UNIX_EPOCH).unwrap().as_secs(),
            1_794_859_210
        );

        let offset = parse_rfc3339("2026-11-16T12:00:10.123456789-08:00").unwrap();
        assert_eq!(
            offset.duration_since(UNIX_EPOCH).unwrap(),
            Duration::new(1_794_859_210, 123_456_789)
        );

        let short_fraction = parse_rfc3339("2026-11-16T20:00:10.5+00:00").unwrap();
        assert_eq!(
            short_fraction.duration_since(UNIX_EPOCH).unwrap(),
            Duration::new(1_794_859_210, 500_000_000)
        );

        let before_epoch = parse_rfc3339("1969-12-31T23:59:59Z").unwrap();
        assert_eq!(
            UNIX_EPOCH.duration_since(before_epoch).unwrap(),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn rejects_bad_rfc3339() {
        for text in [
            "",
            "2026-11-16",
            "2026-11-16T20:00:10",
            "2026-13-16T20:00:10Z",
            "2026-11-16T20:00:10+0800",
        ] {
            assert_eq!(parse_rfc3339(text), None, "{text} should not parse");
        }
    }
}
