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

#[must_use]
pub fn format_time_styled(time: SystemTime, dim: Style) -> String {
    use std::fmt::Write;
    let plain = format_time(time);
    let mut out = String::with_capacity(plain.len() + 20);
    // Dim the `T` separator and trailing `Z` zone marker so the numeric
    // fields stand out. Matching on the byte itself instead of a hard-coded
    // index lets this stay correct for any year width jiff produces.
    for b in plain.as_bytes() {
        let dimmed = *b == b'T' || *b == b'Z';
        if dimmed {
            let _ = write!(out, "{dim}");
        }
        out.push(*b as char);
        if dimmed {
            let _ = write!(out, "{}", dim.render_reset());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{format_time, format_time_styled};
    use anstyle::{Effects, Style};
    use std::time::{Duration, SystemTime};

    #[test]
    fn epoch_renders_iso_zulu() {
        assert_eq!(format_time(SystemTime::UNIX_EPOCH), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn known_timestamp_round_trips() {
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_777_628_417);
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
    fn styled_output_dims_t_and_z() {
        let dim = Style::new().effects(Effects::DIMMED);
        let styled = format_time_styled(SystemTime::UNIX_EPOCH, dim);
        let plain = format_time(SystemTime::UNIX_EPOCH);
        assert!(styled.len() > plain.len());
        let stripped: String = styled
            .replace(&format!("{dim}"), "")
            .replace(&format!("{}", dim.render_reset()), "");
        assert_eq!(stripped, plain);
    }
}
