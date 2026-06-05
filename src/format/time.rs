// Copyright © 2026 Michael Shields
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::time::SystemTime;

use anstyle::Style;
use jiff::Timestamp;

#[must_use]
pub fn format_time(time: SystemTime) -> String {
    let secs = match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
        Err(e) => i64::try_from(e.duration().as_secs())
            .map_or(i64::MIN, |s| s.checked_neg().unwrap_or(i64::MIN)),
    };
    Timestamp::from_second(secs).map_or_else(
        |_| "0000-00-00T00:00:00Z".to_string(),
        |ts| ts.strftime("%Y-%m-%dT%H:%M:%SZ").to_string(),
    )
}

const HOUR: u64 = 3600;
const DAY: u64 = 24 * HOUR;
// Approximate six months as 180 days. The exact boundary doesn't matter
// visually — this tier is a soft age cue, not a precise calendar comparison.
const SIX_MONTHS: u64 = 180 * DAY;

/// Decompose `YYYY[Y...]-MM-DDTHH:MM:SSZ` by separators. Year width is
/// variable (jiff can emit more digits for out-of-range years and a leading
/// `-` for BC years), so split the date fields from the right.
/// Returns the six field substrings or `None` if any separator is missing.
fn parse_iso8601_z(s: &str) -> Option<(&str, &str, &str, &str, &str, &str)> {
    let (date_str, after_t) = s.split_once('T')?;
    let (year_month, day) = date_str.rsplit_once('-')?;
    let (year, month) = year_month.rsplit_once('-')?;
    let (hh, after_hh) = after_t.split_once(':')?;
    let (mm, ss_with_z) = after_hh.split_once(':')?;
    let ss = ss_with_z.strip_suffix('Z')?;
    Some((year, month, day, hh, mm, ss))
}

/// Stale timestamps fade progressively by age so the eye lands on what's fresh.
///
/// Date dim always cascades from the left so the bright remainder is a single
/// run (no "bright middle" between two dim fields):
///   * last 24 hours → year and month (with their trailing hyphens)
///   * 24h–~6 months → year only (with its trailing hyphen)
///   * ≥ ~6 months → date stays fully bright (the differing date is the headline)
///
/// The time portion fades in two steps: `:SS` once the row is at least an hour
/// old; the rest of `HH:MM:SS` once it's at least a day old. The `T` separator
/// and trailing `Z` are dim too while the row is in the past — they carry no
/// information for past rows — but a future mtime renders fully bright so it
/// stands out as anomalous.
///
/// # Panics
///
/// Panics if `format_time`'s output does not match `YYYY-MM-DDTHH:MM:SSZ`.
/// Both of `format_time`'s return paths emit that shape.
#[must_use]
pub fn format_time_styled(time: SystemTime, now: SystemTime, dim: Style) -> String {
    use std::fmt::Write;
    let plain = format_time(time);
    let Ok(past) = now.duration_since(time) else {
        return plain;
    };
    let past_secs = past.as_secs();

    let (year, month, day, hh, mm, ss) =
        parse_iso8601_z(&plain).expect("format_time emits YYYY-MM-DDTHH:MM:SSZ");

    let segs: [(&str, bool); 12] = [
        (year, past_secs < SIX_MONTHS),
        ("-", past_secs < SIX_MONTHS),
        (month, past_secs < DAY),
        ("-", past_secs < DAY),
        (day, false),
        ("T", true),
        (hh, past_secs >= DAY),
        (":", past_secs >= DAY),
        (mm, past_secs >= DAY),
        (":", past_secs >= HOUR),
        (ss, past_secs >= HOUR),
        ("Z", true),
    ];

    let mut out = String::with_capacity(plain.len() + 8);
    let mut opened = false;
    for (text, dim_seg) in segs {
        if dim_seg && !opened {
            let _ = write!(out, "{dim}");
            opened = true;
        } else if !dim_seg && opened {
            let _ = write!(out, "{}", dim.render_reset());
            opened = false;
        }
        out.push_str(text);
    }
    if opened {
        let _ = write!(out, "{}", dim.render_reset());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{format_time, format_time_styled, parse_iso8601_z};
    use anstyle::{Effects, Style};
    use std::time::{Duration, SystemTime};

    #[test]
    fn parse_iso8601_z_accepts_well_formed_input() {
        assert_eq!(
            parse_iso8601_z("2026-05-01T09:40:17Z"),
            Some(("2026", "05", "01", "09", "40", "17"))
        );
    }

    #[test]
    fn parse_iso8601_z_keeps_negative_year_sign() {
        // jiff emits a leading `-` for BC years. The year segment must keep
        // the sign; month/day must not absorb it.
        assert_eq!(
            parse_iso8601_z("-0044-03-15T12:00:00Z"),
            Some(("-0044", "03", "15", "12", "00", "00"))
        );
    }

    #[test]
    fn parse_iso8601_z_rejects_missing_separators() {
        assert_eq!(parse_iso8601_z(""), None); // no T
        assert_eq!(parse_iso8601_z("2026T09:40:17Z"), None); // no first -
        assert_eq!(parse_iso8601_z("2026-05T09:40:17Z"), None); // no second -
        assert_eq!(parse_iso8601_z("2026-05-01T0940:17Z"), None); // no first :
        assert_eq!(parse_iso8601_z("2026-05-01T09:4017Z"), None); // no second :
        assert_eq!(parse_iso8601_z("2026-05-01T09:40:17"), None); // no trailing Z
    }

    fn dim() -> Style {
        Style::new().effects(Effects::DIMMED)
    }

    fn open() -> String {
        format!("{}", dim())
    }

    fn close() -> String {
        format!("{}", dim().render_reset())
    }

    fn strip(s: &str) -> String {
        s.replace(&open(), "").replace(&close(), "")
    }

    const T_2026_05_01: u64 = 1_777_628_417; // 2026-05-01T09:40:17Z

    #[test]
    fn epoch_renders_iso_zulu() {
        assert_eq!(format_time(SystemTime::UNIX_EPOCH), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn known_timestamp_round_trips() {
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(T_2026_05_01);
        assert_eq!(format_time(t), "2026-05-01T09:40:17Z");
    }

    #[test]
    fn pre_epoch_renders_correctly() {
        let t = SystemTime::UNIX_EPOCH - Duration::from_secs(1);
        assert_eq!(format_time(t), "1969-12-31T23:59:59Z");
    }

    #[test]
    fn timestamp_out_of_range_renders_zero_string() {
        let huge = SystemTime::UNIX_EPOCH + Duration::from_secs(300_000_000_017);
        assert_eq!(format_time(huge), "0000-00-00T00:00:00Z");
    }

    #[test]
    fn styled_strips_back_to_plain_format() {
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(T_2026_05_01);
        let now = time + Duration::from_secs(30);
        let styled = format_time_styled(time, now, dim());
        assert_eq!(strip(&styled), format_time(time));
    }

    #[test]
    fn styled_dims_year_and_month_within_24h() {
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(T_2026_05_01);
        let now = time + Duration::from_mins(30);
        let styled = format_time_styled(time, now, dim());
        assert_eq!(
            styled,
            format!(
                "{o}2026-05-{c}01{o}T{c}09:40:17{o}Z{c}",
                o = open(),
                c = close()
            ),
        );
    }

    #[test]
    fn styled_dims_seconds_once_at_least_an_hour_old() {
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(T_2026_05_01);
        let now = time + Duration::from_hours(2);
        let styled = format_time_styled(time, now, dim());
        assert_eq!(
            styled,
            format!(
                "{o}2026-05-{c}01{o}T{c}09:40{o}:17Z{c}",
                o = open(),
                c = close()
            ),
        );
    }

    #[test]
    fn styled_dims_only_year_just_under_six_months() {
        // 24h–6mo tier: year + trailing hyphen dim; month/day bright; full
        // time dim. One second shy of the cutoff, to pin its lower edge.
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(T_2026_05_01);
        let now = time + Duration::from_hours(180 * 24) - Duration::from_secs(1);
        let styled = format_time_styled(time, now, dim());
        assert_eq!(
            styled,
            format!("{o}2026-{c}05-01{o}T09:40:17Z{c}", o = open(), c = close()),
        );
    }

    #[test]
    fn styled_renders_future_timestamps_fully_bright() {
        // Future mtimes are anomalous (clock skew, copied-from-elsewhere file);
        // the visual contrast against the normal dim styling helps catch them.
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(T_2026_05_01);
        let now = time - Duration::from_hours(2);
        let styled = format_time_styled(time, now, dim());
        assert_eq!(styled, "2026-05-01T09:40:17Z");
    }

    #[test]
    fn styled_leaves_date_bright_once_six_months_old() {
        // Exactly at the 180-day cutoff the full date undims; with the
        // just-under test this pins the cutoff value and its boundary side.
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(T_2026_05_01);
        let now = time + Duration::from_hours(180 * 24);
        let styled = format_time_styled(time, now, dim());
        assert_eq!(
            styled,
            format!("2026-05-01{o}T09:40:17Z{c}", o = open(), c = close()),
        );
    }
}
