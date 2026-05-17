use crate::entry::EntryKind;

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
// file or directory whose perms match what the process umask would produce
// (file = 0o666 & ~umask, dir = 0o777 & ~umask), or a symlink at the
// platform's default (777 on Linux, 755 on macOS/BSD — kernel-imposed, not
// umask-derived). Setuid/setgid/sticky always stay bright; so do device,
// FIFO, socket, and unknown kinds.
#[must_use]
pub const fn is_default(kind: EntryKind, mode: u32, umask: u32) -> bool {
    let bits = mode & 0o7777;
    if bits & 0o7000 != 0 {
        return false;
    }
    let perm = bits & 0o0777;
    let umask = umask & 0o0777;
    match kind {
        EntryKind::RegularFile => perm == 0o666 & !umask & 0o777,
        EntryKind::Directory => perm == 0o777 & !umask & 0o777,
        EntryKind::Symlink => perm == 0o755 || perm == 0o777,
        EntryKind::CharDevice
        | EntryKind::BlockDevice
        | EntryKind::Fifo
        | EntryKind::Socket
        | EntryKind::Other => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{EntryKind, format_perms, is_default};

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
    fn is_default_matches_umask_022_defaults() {
        assert!(is_default(EntryKind::RegularFile, 0o644, 0o022));
        assert!(is_default(EntryKind::Directory, 0o755, 0o022));
        // Symlink defaults differ by platform: macOS/BSD writes 755, Linux 777.
        assert!(is_default(EntryKind::Symlink, 0o755, 0o022));
        assert!(is_default(EntryKind::Symlink, 0o777, 0o022));
    }

    #[test]
    fn is_default_tracks_other_umasks() {
        // Restrictive umask 077: files default to 600, dirs to 700.
        assert!(is_default(EntryKind::RegularFile, 0o600, 0o077));
        assert!(is_default(EntryKind::Directory, 0o700, 0o077));
        assert!(!is_default(EntryKind::RegularFile, 0o644, 0o077));
        assert!(!is_default(EntryKind::Directory, 0o755, 0o077));
        // Group-writable umask 002: files default to 664, dirs to 775.
        assert!(is_default(EntryKind::RegularFile, 0o664, 0o002));
        assert!(is_default(EntryKind::Directory, 0o775, 0o002));
        assert!(!is_default(EntryKind::RegularFile, 0o644, 0o002));
        assert!(!is_default(EntryKind::Directory, 0o755, 0o002));
        // Zero umask: files default to 666, dirs to 777.
        assert!(is_default(EntryKind::RegularFile, 0o666, 0));
        assert!(is_default(EntryKind::Directory, 0o777, 0));
    }

    #[test]
    fn is_default_strips_file_type_bits_from_mode() {
        // S_IFREG (0o100000) and S_IFDIR (0o040000) bits must not affect the
        // comparison — only the low 12 bits matter.
        assert!(is_default(EntryKind::RegularFile, 0o100_644, 0o022));
        assert!(is_default(EntryKind::Directory, 0o040_755, 0o022));
    }

    #[test]
    fn is_default_rejects_anything_else() {
        // Wrong perms for the type, even with default umask.
        assert!(!is_default(EntryKind::RegularFile, 0o755, 0o022));
        assert!(!is_default(EntryKind::RegularFile, 0o777, 0o022));
        assert!(!is_default(EntryKind::Directory, 0o644, 0o022));
        assert!(!is_default(EntryKind::Directory, 0o777, 0o022));
        assert!(!is_default(EntryKind::Symlink, 0o644, 0o022));
        // Setuid/setgid/sticky never qualify even on otherwise-default perms.
        assert!(!is_default(EntryKind::RegularFile, 0o4644, 0o022));
        assert!(!is_default(EntryKind::Directory, 0o2755, 0o022));
        assert!(!is_default(EntryKind::Directory, 0o1755, 0o022));
        // Other entry kinds never qualify.
        for k in [
            EntryKind::CharDevice,
            EntryKind::BlockDevice,
            EntryKind::Fifo,
            EntryKind::Socket,
            EntryKind::Other,
        ] {
            assert!(!is_default(k, 0o644, 0o022));
            assert!(!is_default(k, 0o755, 0o022));
            assert!(!is_default(k, 0o777, 0o022));
        }
    }

    #[test]
    fn is_default_ignores_high_bits_in_umask() {
        // umask(2) masks the input to the low 9 bits in the kernel; mirror
        // that here so spurious upper bits in the captured value can't ever
        // flip the comparison.
        assert!(is_default(EntryKind::RegularFile, 0o644, 0o7_777_022));
    }
}
