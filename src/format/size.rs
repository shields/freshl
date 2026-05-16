use anstyle::Style;

// Sizes render as raw digits with no separator. To keep the magnitude scannable
// without breaking copy/paste, digits past the leading six-digit-aligned group
// are dimmed: `2955712` shows the `2` bright and `955712` dim, so the eye
// catches the megabyte boundary the way `_` once did but the text is still a
// plain integer. Returns the visual width separately because the styled string
// carries ANSI escapes that would inflate `chars().count()`.
#[must_use]
pub fn format_size(size: u64, dim: Style) -> (String, usize) {
    use std::fmt::Write;
    let digits = size.to_string();
    let width = digits.len();
    if width <= 6 {
        return (digits, width);
    }
    let head_len = match width % 6 {
        0 => 6,
        r => r,
    };
    let (head, tail) = digits.split_at(head_len);
    let mut out = String::with_capacity(width + 12);
    out.push_str(head);
    let _ = write!(out, "{dim}{tail}{}", dim.render_reset());
    (out, width)
}

#[cfg(test)]
mod tests {
    use super::format_size;
    use anstyle::{Effects, Style};

    fn dim() -> Style {
        Style::new().effects(Effects::DIMMED)
    }

    fn strip(s: &str) -> String {
        let d = dim();
        s.replace(&format!("{d}"), "")
            .replace(&format!("{}", d.render_reset()), "")
    }

    #[test]
    fn small_sizes_render_plain_with_no_styling() {
        for n in [0u64, 1, 123, 999_999] {
            let (s, w) = format_size(n, dim());
            assert_eq!(s, n.to_string());
            assert_eq!(w, s.len());
        }
    }

    #[test]
    fn sizes_above_a_million_dim_trailing_six_digits() {
        let (s, w) = format_size(2_955_712, dim());
        assert_eq!(strip(&s), "2955712");
        assert_eq!(w, 7);
        assert!(s.starts_with('2'));
        assert!(s.contains(&format!("{}955712", dim())));
    }

    #[test]
    fn split_falls_on_six_digit_boundary_from_the_right() {
        let (s, w) = format_size(12_345_678, dim());
        assert_eq!(strip(&s), "12345678");
        assert_eq!(w, 8);
        // First group "12" stays plain, "345678" is dimmed.
        let d = dim();
        let expected = format!("12{d}345678{}", d.render_reset());
        assert_eq!(s, expected);
    }

    #[test]
    fn exactly_six_digits_stays_plain() {
        let (s, w) = format_size(999_999, dim());
        assert_eq!(s, "999999");
        assert_eq!(w, 6);
    }

    #[test]
    fn twelve_digits_splits_six_and_six() {
        let (s, w) = format_size(999_999_999_999, dim());
        assert_eq!(strip(&s), "999999999999");
        assert_eq!(w, 12);
        let d = dim();
        let expected = format!("999999{d}999999{}", d.render_reset());
        assert_eq!(s, expected);
    }

    #[test]
    fn thirteen_digits_dims_all_twelve_trailing_digits_as_one_run() {
        let (s, w) = format_size(1_234_567_890_123, dim());
        assert_eq!(strip(&s), "1234567890123");
        assert_eq!(w, 13);
        let d = dim();
        let expected = format!("1{d}234567890123{}", d.render_reset());
        assert_eq!(s, expected);
    }
}
