#[must_use]
pub fn format_perms(mode: u32) -> String {
    let bits = mode & 0o7777;
    if bits & 0o7000 == 0 {
        format!("{bits:03o}")
    } else {
        format!("{bits:04o}")
    }
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
}
