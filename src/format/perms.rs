#[must_use]
pub fn format_perms(mode: u32) -> String {
    let bits = mode & 0o7777;
    if bits & 0o7000 == 0 {
        format!("{bits:03o}")
    } else {
        format!("{bits:04o}")
    }
}

// The type+mode column is dimmed when it carries no information: a regular
// file at 644, a directory at 755, or a symlink at the platform's default
// (777 on Linux, 755 on macOS/BSD). Any other combination — odd perms,
// setuid, world-writable — stays bright so the eye lands on it.
#[must_use]
pub fn is_default(kind: char, mode: &str) -> bool {
    matches!(
        (kind, mode),
        (' ', "644") | ('d', "755") | ('l', "755" | "777"),
    )
}

#[cfg(test)]
mod tests {
    use super::format_perms;

    #[test]
    fn standard_permissions_render_three_digits() {
        assert_eq!(format_perms(0o755), "755");
        assert_eq!(format_perms(0o644), "644");
        assert_eq!(format_perms(0o600), "600");
        assert_eq!(format_perms(0o000), "000");
        assert_eq!(format_perms(0o022), "022");
    }

    #[test]
    fn sticky_setuid_setgid_render_four_digits() {
        assert_eq!(format_perms(0o4755), "4755");
        assert_eq!(format_perms(0o2755), "2755");
        assert_eq!(format_perms(0o1777), "1777");
        assert_eq!(format_perms(0o7777), "7777");
    }

    #[test]
    fn ignores_file_type_bits() {
        assert_eq!(format_perms(0o100_644), "644");
        assert_eq!(format_perms(0o040_755), "755");
    }

    #[test]
    fn is_default_matches_boring_combinations() {
        assert!(super::is_default(' ', "644"));
        assert!(super::is_default('d', "755"));
        // Symlink defaults differ by platform: macOS/BSD writes 755, Linux 777.
        assert!(super::is_default('l', "755"));
        assert!(super::is_default('l', "777"));
    }

    #[test]
    fn is_default_rejects_anything_else() {
        // Wrong perms for the type.
        assert!(!super::is_default(' ', "755"));
        assert!(!super::is_default(' ', "777"));
        assert!(!super::is_default('d', "644"));
        assert!(!super::is_default('d', "777"));
        assert!(!super::is_default('l', "644"));
        // Non-default perms.
        assert!(!super::is_default(' ', "600"));
        assert!(!super::is_default('d', "700"));
        assert!(!super::is_default(' ', "4755"));
        // Other entry kinds never qualify.
        for k in ['c', 'b', 'p', 's', '?'] {
            assert!(!super::is_default(k, "644"));
            assert!(!super::is_default(k, "755"));
            assert!(!super::is_default(k, "777"));
        }
    }
}
