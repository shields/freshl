// Sizes are grouped in clusters of six digits separated by `_`, per
// docs/plan.md. This is a deliberate deviation from the usual three-digit
// thousands separator: six-digit clusters align to megabyte/terabyte
// boundaries (`1_234567` vs `1,234,567`) which lets the eye scan exact byte
// counts quickly.
#[must_use]
pub fn format_size(size: u64) -> String {
    let digits = size.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 6);
    let bytes = digits.as_bytes();
    let n = bytes.len();
    for (i, b) in bytes.iter().enumerate() {
        let remaining = n - i;
        // `is_multiple_of` is stable on `usize` since Rust 1.84; the
        // `rust-version = "1.95"` in Cargo.toml guarantees its availability.
        if i > 0 && remaining.is_multiple_of(6) {
            out.push('_');
        }
        out.push(*b as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::format_size;

    #[test]
    fn small_sizes_have_no_separator() {
        assert_eq!(format_size(0), "0");
        assert_eq!(format_size(1), "1");
        assert_eq!(format_size(123), "123");
        assert_eq!(format_size(999_999), "999999");
    }

    #[test]
    fn sizes_above_a_million_get_one_separator() {
        assert_eq!(format_size(1_000_000), "1_000000");
        assert_eq!(format_size(12_345_678), "12_345678");
        assert_eq!(format_size(999_999_999_999), "999999_999999");
    }

    #[test]
    fn sizes_above_a_trillion_get_two_separators() {
        assert_eq!(format_size(1_000_000_000_000), "1_000000_000000");
        assert_eq!(format_size(1_234_567_890_123), "1_234567_890123");
    }
}
