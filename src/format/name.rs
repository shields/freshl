use std::io::Write;
use std::os::unix::ffi::OsStrExt;

use anstyle::{AnsiColor, Color, Effects, Style};

use crate::entry::{Entry, EntryKind};

#[must_use]
pub fn style_for_kind(kind: EntryKind) -> Style {
    let color = match kind {
        EntryKind::Directory => Some(Color::Ansi(AnsiColor::Blue)),
        EntryKind::Symlink => Some(Color::Ansi(AnsiColor::Cyan)),
        EntryKind::CharDevice | EntryKind::BlockDevice => Some(Color::Ansi(AnsiColor::Yellow)),
        EntryKind::Fifo => Some(Color::Ansi(AnsiColor::Magenta)),
        EntryKind::Socket => Some(Color::Ansi(AnsiColor::Green)),
        EntryKind::RegularFile | EntryKind::Other => None,
    };
    color.map_or_else(Style::new, |c| Style::new().fg_color(Some(c)))
}

/// Render the styled name as raw bytes.
///
/// ANSI escape sequences are interleaved with the filename's underlying bytes
/// verbatim, so non-UTF-8 names round-trip exactly to a pipe while still
/// rendering with the right colour on a terminal. For symlinks, the output
/// includes a dimmed arrow and the link target.
#[must_use]
pub fn format_name(entry: &Entry, dim_if_ignored: bool, target_missing: bool) -> Vec<u8> {
    let kind_style = style_for_kind(entry.kind);
    let dim = Style::new().effects(Effects::DIMMED);
    let red = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Red)));

    let mut out = Vec::with_capacity(entry.name.len() + 32);
    let name_style = if dim_if_ignored {
        kind_style.effects(Effects::DIMMED)
    } else {
        kind_style
    };
    let _ = write!(out, "{name_style}");
    out.extend_from_slice(entry.name.as_bytes());
    let _ = write!(out, "{}", name_style.render_reset());

    if let Some(target) = &entry.symlink_target {
        let _ = write!(out, " {dim}→{} ", dim.render_reset());
        let style = if target_missing { red } else { dim };
        let _ = write!(out, "{style}");
        out.extend_from_slice(target.as_os_str().as_bytes());
        let _ = write!(out, "{}", style.render_reset());
    }

    out
}

#[cfg(test)]
mod tests {
    use super::{format_name, style_for_kind};
    use crate::entry::{Entry, EntryKind};
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::time::SystemTime;

    fn entry(name: &str, kind: EntryKind) -> Entry {
        Entry {
            name: OsString::from(name),
            path: PathBuf::from(name),
            kind,
            mode: 0,
            nlink: 0,
            uid: 0,
            gid: 0,
            size: 0,
            rdev: 0,
            mtime: SystemTime::UNIX_EPOCH,
            symlink_target: None,
        }
    }

    fn as_lossy(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    #[test]
    fn style_for_directory_uses_blue() {
        let s = style_for_kind(EntryKind::Directory);
        assert!(format!("{s}").contains("34"));
    }

    #[test]
    fn style_for_regular_file_is_default() {
        let s = style_for_kind(EntryKind::RegularFile);
        assert_eq!(format!("{s}"), String::new());
    }

    #[test]
    fn style_for_every_kind() {
        for k in [
            EntryKind::Directory,
            EntryKind::Symlink,
            EntryKind::CharDevice,
            EntryKind::BlockDevice,
            EntryKind::Fifo,
            EntryKind::Socket,
            EntryKind::RegularFile,
            EntryKind::Other,
        ] {
            let _ = style_for_kind(k);
        }
    }

    #[test]
    fn formats_plain_file_name() {
        let e = entry("hello", EntryKind::RegularFile);
        let bytes = format_name(&e, false, false);
        assert!(as_lossy(&bytes).contains("hello"));
    }

    #[test]
    fn formats_symlink_with_arrow_and_target() {
        let mut e = entry("link", EntryKind::Symlink);
        e.symlink_target = Some(PathBuf::from("/usr/bin"));
        let bytes = format_name(&e, false, false);
        let s = as_lossy(&bytes);
        assert!(s.contains("link"));
        assert!(s.contains('→'));
        assert!(s.contains("/usr/bin"));
    }

    #[test]
    fn missing_target_uses_red() {
        let mut e = entry("link", EntryKind::Symlink);
        e.symlink_target = Some(PathBuf::from("nowhere"));
        let plain = format_name(&e, false, false);
        let red_styled = format_name(&e, false, true);
        assert_ne!(plain, red_styled);
        assert!(as_lossy(&red_styled).contains("nowhere"));
    }

    #[test]
    fn ignored_files_get_dim_style() {
        let e = entry("ignored", EntryKind::RegularFile);
        let dim = format_name(&e, true, false);
        let plain = format_name(&e, false, false);
        assert_ne!(plain, dim);
    }

    #[test]
    fn non_utf8_name_round_trips_exactly() {
        use std::os::unix::ffi::OsStringExt;
        let raw = vec![b'b', b'a', b'd', 0xFF, b'8'];
        let mut e = Entry {
            name: OsString::from_vec(raw.clone()),
            path: PathBuf::from("bad"),
            kind: EntryKind::RegularFile,
            mode: 0,
            nlink: 0,
            uid: 0,
            gid: 0,
            size: 0,
            rdev: 0,
            mtime: SystemTime::UNIX_EPOCH,
            symlink_target: None,
        };
        e.symlink_target = None;
        let bytes = format_name(&e, false, false);
        // The raw byte 0xFF must appear in the output verbatim, not as U+FFFD.
        assert!(bytes.windows(raw.len()).any(|w| w == raw.as_slice()));
    }
}
