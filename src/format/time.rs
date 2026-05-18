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
// Approximate one year as 365 days. The exact boundary doesn't matter
// visually — this tier is a soft age cue, not a precise calendar comparison.
const YEAR: u64 = 365 * DAY;

/// Stale timestamps fade progressively by age so the eye lands on what's fresh.
///
/// Date dim always cascades from the left so the bright remainder is a single
/// run (no "bright middle" between two dim fields):
///   * last 24 hours → year and month (with their trailing hyphens)
///   * 24h–~1 year  → year only (with its trailing hyphen)
///   * ≥ ~1 year → date stays fully bright (the differing date is the headline)
///
/// The time portion fades in two steps: `:SS` once the row is at least an hour
/// old; the rest of `HH:MM:SS` once it's at least a day old. The `T` separator
/// and trailing `Z` are dim too while the row is in the past — they carry no
/// information for past rows — but a future mtime renders fully bright so it
/// stands out as anomalous.
///
/// # Panics
///
/// Panics if `format_time`'s output lacks `T` between the date and time fields.
/// This never happens: both of its return paths emit `T`.
#[must_use]
pub fn format_time_styled(time: SystemTime, now: SystemTime, dim: Style) -> String {
    use std::fmt::Write;
    let plain = format_time(time);
    let bytes = plain.as_bytes();
    let len = bytes.len();
    let past_secs = match now.duration_since(time) {
        Ok(d) => d.as_secs(),
        Err(_) => return plain,
    };

    // Year width is variable (jiff zero-pads to 4 digits but can emit more for
    // out-of-range years), so locate `T` rather than indexing from either end.
    // The invariant is enforced by `format_time` itself, whose only two return
    // paths both emit `T` between the date and time fields.
    let t_pos = plain.find('T').expect("format_time always emits 'T'");
    let year_end = t_pos - 5; // year + `-`
    let month_end = t_pos - 2; // year + `-` + month + `-`

    let mut mask = vec![false; len];
    mask[t_pos] = true;
    mask[len - 1] = true;
    if past_secs < DAY {
        mask[..month_end].fill(true);
    } else if past_secs < YEAR {
        mask[..year_end].fill(true);
    }
    if past_secs >= DAY {
        mask[t_pos + 1..(t_pos + 9).min(len)].fill(true);
    } else if past_secs >= HOUR {
        mask[t_pos + 6..(t_pos + 9).min(len)].fill(true);
    }

    let mut out = String::with_capacity(len + 8);
    let mut opened = false;
    for (i, &b) in bytes.iter().enumerate() {
        if mask[i] && !opened {
            let _ = write!(out, "{dim}");
            opened = true;
        } else if !mask[i] && opened {
            let _ = write!(out, "{}", dim.render_reset());
            opened = false;
        }
        out.push(b as char);
    }
    if opened {
        let _ = write!(out, "{}", dim.render_reset());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{format_time, format_time_styled};
    use anstyle::{Effects, Style};
    use std::time::{Duration, SystemTime};

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
    fn styled_dims_only_year_within_a_year() {
        // 24h–1y tier: year + trailing hyphen dim; month/day bright; full time dim.
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(T_2026_05_01);
        let now = time + Duration::from_hours(7 * 24);
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
    fn styled_leaves_date_bright_beyond_a_year() {
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(T_2026_05_01);
        let now = time + Duration::from_hours(400 * 24);
        let styled = format_time_styled(time, now, dim());
        assert_eq!(
            styled,
            format!("2026-05-01{o}T09:40:17Z{c}", o = open(), c = close()),
        );
    }
}
