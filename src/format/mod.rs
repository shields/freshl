use std::io::Write;
use std::time::SystemTime;

use anstyle::{Effects, Style};

use crate::entry::{Entry, EntryKind};
use crate::format::palette::Palette;
use crate::owner::{OwnerCache, UserDirectory};

pub mod git_col;
pub mod name;
pub mod palette;
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
    /// Wrap kind+mode in dim escapes at render time when this row's perms
    /// are the boring default (regular/644, dir/755, symlink/755) — keeps
    /// the eye on rows with anything unusual.
    pub dim_mode: bool,
    pub nlink: String,
    /// Wrap `nlink` in dim escapes at render time when the count is 1 — the
    /// overwhelming default for regular files, so dimming it lets the eye
    /// catch hardlinked entries.
    pub dim_nlink: bool,
    pub owner: String,
    pub group: String,
    /// Wrap the group column in dim escapes at render time when the gid is
    /// the owner's primary group from passwd — that column then carries no
    /// extra information, so dimming it lets the eye catch rows where the
    /// file's group differs from the owner's default.
    pub dim_group: bool,
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

pub fn build_row<D: UserDirectory>(
    entry: &Entry,
    owners: &mut OwnerCache<D>,
    palette: &Palette,
    now: SystemTime,
) -> Row {
    let dim = Style::new().effects(Effects::DIMMED);
    let (size, size_width) = match entry.kind {
        EntryKind::CharDevice | EntryKind::BlockDevice => size::format_rdev(entry.rdev),
        _ => size::format_size(entry.size, dim),
    };
    let kind = entry.kind.type_char();
    let mode = perms::format_perms(entry.mode);
    let dim_mode = perms::is_default(kind, &mode);
    let dim_group = owners.gid_is_primary(entry.uid, entry.gid);
    Row {
        kind,
        mode,
        dim_mode,
        nlink: entry.nlink.to_string(),
        dim_nlink: entry.nlink == 1,
        owner: owners.user(entry.uid).to_string_lossy().into_owned(),
        group: owners.group(entry.gid).to_string_lossy().into_owned(),
        dim_group,
        size,
        size_width,
        mtime: time::format_time_styled(entry.mtime, now, dim),
        git: None,
        name: name::format_name(palette, entry, false, false),
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
    rows.iter()
        .map(|r| field(r).chars().count())
        .max()
        .unwrap_or(0)
}

/// Wrap `body`'s output in dim-open/reset escapes when `on` is true.
fn wrap_dim(out: &mut Vec<u8>, on: bool, dim: Style, body: impl FnOnce(&mut Vec<u8>)) {
    if on {
        let _ = write!(out, "{dim}");
    }
    body(out);
    if on {
        let _ = write!(out, "{}", dim.render_reset());
    }
}

#[must_use]
pub fn render_row(row: &Row, widths: ColumnWidths, git_width: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(row.name.len() + 96);
    let dim = Style::new().effects(Effects::DIMMED);
    wrap_dim(&mut out, row.dim_mode, dim, |out| {
        let _ = write!(out, "{}", row.kind);
        let _ = write!(out, "{:>w$}", row.mode, w = widths.mode);
    });
    out.push(b' ');
    wrap_dim(&mut out, row.dim_nlink, dim, |out| {
        let _ = write!(out, "{:>w$}", row.nlink, w = widths.nlink);
    });
    out.push(b' ');
    let _ = write!(out, "{:<w$} ", row.owner, w = widths.owner);
    wrap_dim(&mut out, row.dim_group, dim, |out| {
        let _ = write!(out, "{:<w$}", row.group, w = widths.group);
    });
    out.push(b' ');
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
    use crate::format::palette::Palette;
    use crate::owner::{OwnerCache, UserDirectory, UserRecord};
    use anstyle::{Effects, Style};
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::time::SystemTime;

    struct Fixed;
    impl UserDirectory for Fixed {
        fn lookup_user(&self, _uid: u32) -> Option<UserRecord> {
            Some(UserRecord {
                name: OsString::from("alice"),
                primary_gid: 20,
            })
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
            symlink_target_is_dir: false,
        }
    }

    #[test]
    fn build_row_renders_device_rdev_in_size_column() {
        let mut owners = OwnerCache::new(Fixed);
        let palette = Palette::empty();
        for (kind, rdev, expected, kind_char) in [
            (EntryKind::CharDevice, 0x0300_0002u64, "0x3000002", 'c'),
            (EntryKind::BlockDevice, 0x0100_0000u64, "0x1000000", 'b'),
        ] {
            let mut e = entry("dev");
            e.kind = kind;
            e.size = 0;
            e.rdev = rdev;
            let row = build_row(&e, &mut owners, &palette, SystemTime::UNIX_EPOCH);
            assert_eq!(row.size, expected);
            assert_eq!(row.size_width, expected.len());
            assert_eq!(row.kind, kind_char);
        }
    }

    #[test]
    fn build_row_populates_basic_fields() {
        let mut owners = OwnerCache::new(Fixed);
        let palette = Palette::empty();
        let row = build_row(&entry("hi"), &mut owners, &palette, SystemTime::UNIX_EPOCH);
        assert_eq!(row.kind, ' ');
        assert_eq!(row.mode, "644");
        assert!(row.dim_mode);
        assert_eq!(row.nlink, "1");
        assert!(row.dim_nlink);
        assert_eq!(row.owner, "alice");
        assert_eq!(row.group, "staff");
        assert!(row.dim_group);
        assert_eq!(row.size, "1234");
        assert!(row.mtime.contains("1970-01-"));
        assert!(row.mtime.contains("00:00:00"));
    }

    #[test]
    fn build_row_clears_dim_mode_for_unusual_perms() {
        let mut owners = OwnerCache::new(Fixed);
        let palette = Palette::empty();
        let mut e = entry("hi");
        e.mode = 0o100_600;
        let row = build_row(&e, &mut owners, &palette, SystemTime::UNIX_EPOCH);
        assert_eq!(row.mode, "600");
        assert!(!row.dim_mode);
    }

    #[test]
    fn build_row_clears_dim_nlink_for_hardlinked_entry() {
        let mut owners = OwnerCache::new(Fixed);
        let palette = Palette::empty();
        let mut e = entry("hi");
        e.nlink = 2;
        let row = build_row(&e, &mut owners, &palette, SystemTime::UNIX_EPOCH);
        assert_eq!(row.nlink, "2");
        assert!(!row.dim_nlink);
    }

    #[test]
    fn build_row_clears_dim_group_when_gid_is_not_primary() {
        let mut owners = OwnerCache::new(Fixed);
        let palette = Palette::empty();
        let mut e = entry("hi");
        e.gid = 30;
        let row = build_row(&e, &mut owners, &palette, SystemTime::UNIX_EPOCH);
        assert!(!row.dim_group);
    }

    #[test]
    fn build_row_clears_dim_group_when_owner_unknown() {
        struct NoUser;
        impl UserDirectory for NoUser {
            fn lookup_user(&self, _uid: u32) -> Option<UserRecord> {
                None
            }
            fn group_name(&self, _gid: u32) -> Option<OsString> {
                Some(OsString::from("staff"))
            }
        }
        let mut owners = OwnerCache::new(NoUser);
        let palette = Palette::empty();
        let row = build_row(&entry("hi"), &mut owners, &palette, SystemTime::UNIX_EPOCH);
        assert!(!row.dim_group);
    }

    #[test]
    fn build_row_sets_dim_mode_for_default_directory() {
        let mut owners = OwnerCache::new(Fixed);
        let palette = Palette::empty();
        let mut e = entry("d");
        e.kind = EntryKind::Directory;
        e.mode = 0o040_755;
        let row = build_row(&e, &mut owners, &palette, SystemTime::UNIX_EPOCH);
        assert_eq!(row.kind, 'd');
        assert_eq!(row.mode, "755");
        assert!(row.dim_mode);
    }

    #[test]
    fn compute_widths_finds_maximum_of_each_column() {
        let rows = vec![
            Row {
                kind: '-',
                mode: "644".into(),
                dim_mode: false,
                nlink: "1".into(),
                dim_nlink: false,
                owner: "x".into(),
                group: "staff".into(),
                dim_group: false,
                size: "1".into(),
                size_width: 1,
                mtime: "2026".into(),
                git: None,
                name: b"a".to_vec(),
            },
            Row {
                kind: '-',
                mode: "4755".into(),
                dim_mode: false,
                nlink: "99".into(),
                dim_nlink: false,
                owner: "longer".into(),
                group: "g".into(),
                dim_group: false,
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
            dim_mode: false,
            nlink: "2".into(),
            dim_nlink: false,
            owner: "alice".into(),
            group: "staff".into(),
            dim_group: false,
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
    fn render_row_wraps_kind_and_mode_in_dim_when_flagged() {
        let row = Row {
            kind: 'd',
            mode: "755".into(),
            dim_mode: true,
            nlink: "2".into(),
            dim_nlink: false,
            owner: "alice".into(),
            group: "staff".into(),
            dim_group: false,
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
        let dim = Style::new().effects(Effects::DIMMED);
        let open = format!("{dim}");
        let close = format!("{}", dim.render_reset());
        let expected = format!("{open}d 755{close} ");
        assert!(
            s.windows(expected.len())
                .any(|w| w == expected.as_bytes()),
            "row should open dim before 'd', close after '755': {s:?}",
        );
    }

    #[test]
    fn render_row_omits_dim_escapes_when_flag_unset() {
        let row = Row {
            kind: 'd',
            mode: "755".into(),
            dim_mode: false,
            nlink: "2".into(),
            dim_nlink: false,
            owner: "alice".into(),
            group: "staff".into(),
            dim_group: false,
            size: "0".into(),
            size_width: 1,
            mtime: "2026-05-15T11:02:00Z".into(),
            git: None,
            name: b"src".to_vec(),
        };
        let widths = ColumnWidths {
            mode: 3,
            nlink: 1,
            owner: 5,
            group: 5,
            size: 1,
        };
        let s = render_row(&row, widths, 0);
        let dim = Style::new().effects(Effects::DIMMED);
        let open = format!("{dim}");
        assert!(
            !s.windows(open.len()).any(|w| w == open.as_bytes()),
            "no dim escape expected: {s:?}",
        );
    }

    #[test]
    fn render_row_wraps_nlink_in_dim_when_flagged() {
        let row = Row {
            kind: '-',
            mode: "644".into(),
            dim_mode: false,
            nlink: "1".into(),
            dim_nlink: true,
            owner: "alice".into(),
            group: "staff".into(),
            dim_group: false,
            size: "0".into(),
            size_width: 1,
            mtime: "2026-05-15T11:02:00Z".into(),
            git: None,
            name: b"src".to_vec(),
        };
        let widths = ColumnWidths {
            mode: 3,
            nlink: 2,
            owner: 5,
            group: 5,
            size: 1,
        };
        let s = render_row(&row, widths, 0);
        let dim = Style::new().effects(Effects::DIMMED);
        let open = format!("{dim}");
        let close = format!("{}", dim.render_reset());
        let expected = format!("{open} 1{close} ");
        assert!(
            s.windows(expected.len())
                .any(|w| w == expected.as_bytes()),
            "row should open dim before padded nlink, close after: {s:?}",
        );
    }

    #[test]
    fn render_row_wraps_group_in_dim_when_flagged() {
        let row = Row {
            kind: '-',
            mode: "644".into(),
            dim_mode: false,
            nlink: "1".into(),
            dim_nlink: false,
            owner: "alice".into(),
            group: "staff".into(),
            dim_group: true,
            size: "0".into(),
            size_width: 1,
            mtime: "2026-05-15T11:02:00Z".into(),
            git: None,
            name: b"src".to_vec(),
        };
        let widths = ColumnWidths {
            mode: 3,
            nlink: 1,
            owner: 5,
            group: 5,
            size: 1,
        };
        let s = render_row(&row, widths, 0);
        let dim = Style::new().effects(Effects::DIMMED);
        let open = format!("{dim}");
        let close = format!("{}", dim.render_reset());
        let expected = format!("{open}staff{close} ");
        assert!(
            s.windows(expected.len()).any(|w| w == expected.as_bytes()),
            "row should open dim before group, close after: {s:?}",
        );
    }

    #[test]
    fn render_row_emits_git_column_when_width_set() {
        let mut row = Row {
            kind: '-',
            mode: "644".into(),
            dim_mode: false,
            nlink: "1".into(),
            dim_nlink: false,
            owner: "alice".into(),
            group: "staff".into(),
            dim_group: false,
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
            dim_mode: false,
            nlink: "1".into(),
            dim_nlink: false,
            owner: "alice".into(),
            group: "staff".into(),
            dim_group: false,
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
