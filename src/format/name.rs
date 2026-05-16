use std::io::Write;
use std::os::unix::ffi::OsStrExt;

use anstyle::{AnsiColor, Color, Effects, Style};

use crate::entry::Entry;
use crate::format::palette::Palette;

/// Render the styled name as raw bytes.
///
/// ANSI escape sequences are interleaved with the filename's underlying bytes
/// verbatim, so non-UTF-8 names round-trip exactly to a pipe while still
/// rendering with the right colour on a terminal. For symlinks, the output
/// includes a dimmed arrow and the link target.
#[must_use]
pub fn format_name(
    palette: &Palette,
    entry: &Entry,
    dim_if_ignored: bool,
    target_missing: bool,
) -> Vec<u8> {
    let base = palette.style_for(entry, target_missing);
    let dim = Style::new().effects(Effects::DIMMED);
    let red = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Red)));

    let mut out = Vec::with_capacity(entry.name.len() + 32);
    let name_style = if dim_if_ignored {
        // OR DIMMED onto whatever the palette already set (bold, fg, …) so the
        // dim cue doesn't overwrite the type/extension styling.
        base.effects(base.get_effects() | Effects::DIMMED)
    } else {
        base
    };
    let _ = write!(out, "{name_style}");
    out.extend_from_slice(entry.name.as_bytes());
    let _ = write!(out, "{}", name_style.render_reset());

    if let Some(target) = &entry.symlink_target {
        let _ = write!(out, " {dim}→{} ", dim.render_reset());
        let style = if target_missing {
            // Honor the user's `mi` indicator when they've set it; fall back
            // to red so a broken link is still visually obvious.
            palette.style_for_missing_target().unwrap_or(red)
        } else {
            dim
        };
        let _ = write!(out, "{style}");
        out.extend_from_slice(target.as_os_str().as_bytes());
        let _ = write!(out, "{}", style.render_reset());
    }

    out
}

#[cfg(test)]
mod tests {
    use super::format_name;
    use crate::entry::{Entry, EntryKind};
    use crate::format::palette::Palette;
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
            symlink_target_is_dir: false,
        }
    }

    fn as_lossy(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    #[test]
    fn formats_plain_file_name() {
        let palette = Palette::empty();
        let e = entry("hello", EntryKind::RegularFile);
        let bytes = format_name(&palette, &e, false, false);
        assert!(as_lossy(&bytes).contains("hello"));
    }

    #[test]
    fn formats_symlink_with_arrow_and_target() {
        let palette = Palette::empty();
        let mut e = entry("link", EntryKind::Symlink);
        e.symlink_target = Some(PathBuf::from("/usr/bin"));
        let bytes = format_name(&palette, &e, false, false);
        let s = as_lossy(&bytes);
        assert!(s.contains("link"));
        assert!(s.contains('→'));
        assert!(s.contains("/usr/bin"));
    }

    #[test]
    fn missing_target_uses_red_when_mi_unset() {
        let palette = Palette::empty();
        let mut e = entry("link", EntryKind::Symlink);
        e.symlink_target = Some(PathBuf::from("nowhere"));
        let plain = format_name(&palette, &e, false, false);
        let red_styled = format_name(&palette, &e, false, true);
        assert_ne!(plain, red_styled);
        let s = as_lossy(&red_styled);
        assert!(s.contains("nowhere"));
        // SGR 31 is anstyle's red; without `mi` configured the target falls
        // back to freshl's hardcoded red.
        assert!(s.contains("31"), "expected red SGR for missing target: {s}");
    }

    #[test]
    fn missing_target_uses_mi_when_palette_sets_it() {
        let palette = Palette::from_string("mi=01;33");
        let mut e = entry("link", EntryKind::Symlink);
        e.symlink_target = Some(PathBuf::from("nowhere"));
        let s = as_lossy(&format_name(&palette, &e, false, true));
        assert!(s.contains("33"), "expected mi yellow on target: {s}");
        assert!(!s.contains("31"), "freshl red should not appear: {s}");
    }

    #[test]
    fn ignored_files_get_dim_style() {
        let palette = Palette::empty();
        let e = entry("ignored", EntryKind::RegularFile);
        let dim = format_name(&palette, &e, true, false);
        let plain = format_name(&palette, &e, false, false);
        assert_ne!(plain, dim);
    }

    #[test]
    fn ignored_dim_preserves_palette_styling() {
        // With a non-empty palette, the DIMMED overlay must layer onto the
        // base style instead of replacing it.
        let palette = Palette::from_string("di=01;34");
        let e = entry("d", EntryKind::Directory);
        let dim = as_lossy(&format_name(&palette, &e, true, false));
        assert!(dim.contains("34"), "blue fg should survive dim overlay: {dim}");
        assert!(dim.contains('2'), "dim effect should be present: {dim}");
    }

    #[test]
    fn non_utf8_name_round_trips_exactly() {
        use std::os::unix::ffi::OsStringExt;
        let palette = Palette::empty();
        let raw = vec![b'b', b'a', b'd', 0xFF, b'8'];
        let e = Entry {
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
            symlink_target_is_dir: false,
        };
        let bytes = format_name(&palette, &e, false, false);
        // The raw byte 0xFF must appear in the output verbatim, not as U+FFFD.
        assert!(bytes.windows(raw.len()).any(|w| w == raw.as_slice()));
    }
}
