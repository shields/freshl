use std::io::Write;

use anstyle::{Effects, Style};

use crate::entry::{Entry, EntryKind};
use crate::owner::{OwnerCache, UserDirectory};

pub mod git_col;
pub mod name;
pub mod perms;
pub mod size;
pub mod time;

#[derive(Debug, Clone, Copy)]
pub struct ColumnWidths {
    pub mode: usize,
    pub nlink: usize,
    pub owner: usize,
    pub group: usize,
    pub size: usize,
}

#[derive(Debug, Clone)]
pub struct Row {
    pub kind: char,
    pub mode: String,
    pub nlink: String,
    pub owner: String,
    pub group: String,
    pub size: String,
    /// Visible digit count for `size`. Tracked separately because `size`
    /// carries ANSI dim escapes whose bytes would skew `chars().count()`.
    pub size_width: usize,
    pub mtime: String,
    pub git: Option<String>,
    /// Raw bytes for the name column (ANSI escapes interleaved with the
    /// filename's underlying OS bytes), so non-UTF-8 names survive
    /// round-tripping to a pipe.
    pub name: Vec<u8>,
}

pub fn build_row<D: UserDirectory>(entry: &Entry, owners: &mut OwnerCache<D>) -> Row {
    let dim = Style::new().effects(Effects::DIMMED);
    let (size, size_width) = match entry.kind {
        EntryKind::CharDevice | EntryKind::BlockDevice => size::format_rdev(entry.rdev),
        _ => size::format_size(entry.size, dim),
    };
    Row {
        kind: entry.kind.type_char(),
        mode: perms::format_perms(entry.mode),
        nlink: entry.nlink.to_string(),
        owner: owners.user(entry.uid).to_string_lossy().into_owned(),
        group: owners.group(entry.gid).to_string_lossy().into_owned(),
        size,
        size_width,
        mtime: time::format_time_styled(entry.mtime, dim),
        git: None,
        name: name::format_name(entry, false, false),
    }
}

#[must_use]
pub fn compute_widths(rows: &[Row]) -> ColumnWidths {
    // `chars().count()` measures display width via std's `fmt` padding rules
    // (which count chars, not bytes); using `.len()` would over-allocate
    // padding for any owner/group containing multi-byte UTF-8. Size uses its
    // own pre-computed width because its string carries ANSI escapes.
    ColumnWidths {
        mode: max_width(rows, |r| &r.mode),
        nlink: max_width(rows, |r| &r.nlink),
        owner: max_width(rows, |r| &r.owner),
        group: max_width(rows, |r| &r.group),
        size: rows.iter().map(|r| r.size_width).max().unwrap_or(0),
    }
}

fn max_width(rows: &[Row], field: impl Fn(&Row) -> &str) -> usize {
    rows.iter().map(|r| field(r).chars().count()).max().unwrap_or(0)
}

#[must_use]
pub fn render_row(row: &Row, widths: ColumnWidths, git_width: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(row.name.len() + 96);
    let _ = write!(out, "{}", row.kind);
    let _ = write!(out, "{:>w$} ", row.mode, w = widths.mode);
    let _ = write!(out, "{:>w$} ", row.nlink, w = widths.nlink);
    let _ = write!(out, "{:<w$} ", row.owner, w = widths.owner);
    let _ = write!(out, "{:<w$} ", row.group, w = widths.group);
    // Size column carries ANSI escapes, so pad by visual width rather than
    // letting `{:>w$}` count escape bytes as visible characters.
    let pad = widths.size.saturating_sub(row.size_width);
    out.extend(std::iter::repeat_n(b' ', pad));
    out.extend_from_slice(row.size.as_bytes());
    out.push(b' ');
    let _ = write!(out, "{} ", row.mtime);
    if git_width > 0 {
        if let Some(g) = &row.git {
            let _ = write!(out, "{g} ");
        } else {
            out.extend(std::iter::repeat_n(b' ', git_width + 1));
        }
    }
    out.extend_from_slice(&row.name);
    out
}

#[cfg(test)]
mod tests {
    use super::{ColumnWidths, Row, build_row, compute_widths, render_row};
    use crate::entry::{Entry, EntryKind};
    use crate::owner::{OwnerCache, UserDirectory};
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::time::SystemTime;

    struct Fixed;
    impl UserDirectory for Fixed {
        fn user_name(&self, _uid: u32) -> Option<OsString> {
            Some(OsString::from("alice"))
        }
        fn group_name(&self, _gid: u32) -> Option<OsString> {
            Some(OsString::from("staff"))
        }
    }

    fn entry(name: &str) -> Entry {
        Entry {
            name: OsString::from(name),
            path: PathBuf::from(name),
            kind: EntryKind::RegularFile,
            mode: 0o100_644,
            nlink: 1,
            uid: 501,
            gid: 20,
            size: 1234,
            rdev: 0,
            mtime: SystemTime::UNIX_EPOCH,
            symlink_target: None,
        }
    }

    #[test]
    fn build_row_renders_device_rdev_in_size_column() {
        let mut owners = OwnerCache::new(Fixed);
        for (kind, rdev, expected, kind_char) in [
            (EntryKind::CharDevice, 0x0300_0002u64, "0x3000002", 'c'),
            (EntryKind::BlockDevice, 0x0100_0000u64, "0x1000000", 'b'),
        ] {
            let mut e = entry("dev");
            e.kind = kind;
            e.size = 0;
            e.rdev = rdev;
            let row = build_row(&e, &mut owners);
            assert_eq!(row.size, expected);
            assert_eq!(row.size_width, expected.len());
            assert_eq!(row.kind, kind_char);
        }
    }

    #[test]
    fn build_row_populates_basic_fields() {
        let mut owners = OwnerCache::new(Fixed);
        let row = build_row(&entry("hi"), &mut owners);
        assert_eq!(row.kind, ' ');
        assert_eq!(row.mode, "644");
        assert_eq!(row.nlink, "1");
        assert_eq!(row.owner, "alice");
        assert_eq!(row.group, "staff");
        assert_eq!(row.size, "1234");
        assert!(row.mtime.contains("1970-01-01"));
    }

    #[test]
    fn compute_widths_finds_maximum_of_each_column() {
        let rows = vec![
            Row {
                kind: '-',
                mode: "644".into(),
                nlink: "1".into(),
                owner: "x".into(),
                group: "staff".into(),
                size: "1".into(),
                size_width: 1,
                mtime: "2026".into(),
                git: None,
                name: b"a".to_vec(),
            },
            Row {
                kind: '-',
                mode: "4755".into(),
                nlink: "99".into(),
                owner: "longer".into(),
                group: "g".into(),
                size: "1234".into(),
                size_width: 4,
                mtime: "2026".into(),
                git: None,
                name: b"b".to_vec(),
            },
        ];
        let w = compute_widths(&rows);
        assert_eq!(w.mode, 4);
        assert_eq!(w.nlink, 2);
        assert_eq!(w.owner, 6);
        assert_eq!(w.group, 5);
        assert_eq!(w.size, 4);
    }

    #[test]
    fn compute_widths_on_empty_returns_zero_widths() {
        let w = compute_widths(&[]);
        assert_eq!(w.mode, 0);
        assert_eq!(w.size, 0);
    }

    #[test]
    fn render_row_pads_each_column() {
        let row = Row {
            kind: 'd',
            mode: "755".into(),
            nlink: "2".into(),
            owner: "alice".into(),
            group: "staff".into(),
            size: "0".into(),
            size_width: 1,
            mtime: "2026-05-15T11:02:00Z".into(),
            git: None,
            name: b"src".to_vec(),
        };
        let widths = ColumnWidths {
            mode: 4,
            nlink: 2,
            owner: 7,
            group: 5,
            size: 9,
        };
        let s = render_row(&row, widths, 0);
        assert!(s.starts_with(b"d 755  2 alice   staff "));
        assert!(s.ends_with(b"src"));
    }

    #[test]
    fn render_row_emits_git_column_when_width_set() {
        let mut row = Row {
            kind: '-',
            mode: "644".into(),
            nlink: "1".into(),
            owner: "alice".into(),
            group: "staff".into(),
            size: "0".into(),
            size_width: 1,
            mtime: "2026-05-15T11:02:00Z".into(),
            git: Some("M ".into()),
            name: b"file".to_vec(),
        };
        let widths = ColumnWidths {
            mode: 3,
            nlink: 1,
            owner: 5,
            group: 5,
            size: 1,
        };
        let with_git = render_row(&row, widths, 2);
        assert!(with_git.windows(4).any(|w| w == b" M  "));

        row.git = None;
        let blanked = render_row(&row, widths, 2);
        assert!(blanked.windows(4).any(|w| w == b"    "));
    }

    #[test]
    fn render_row_preserves_non_utf8_name_bytes() {
        let row = Row {
            kind: '-',
            mode: "644".into(),
            nlink: "1".into(),
            owner: "alice".into(),
            group: "staff".into(),
            size: "0".into(),
            size_width: 1,
            mtime: "2026-05-15T11:02:00Z".into(),
            git: None,
            name: vec![b'a', 0xFF, b'b'],
        };
        let widths = ColumnWidths {
            mode: 3,
            nlink: 1,
            owner: 5,
            group: 5,
            size: 1,
        };
        let line = render_row(&row, widths, 0);
        assert!(line.contains(&0xFF));
    }
}
