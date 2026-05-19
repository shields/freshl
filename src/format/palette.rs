// Copyright © 2026 Michael Shields
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use anstyle::{Ansi256Color, AnsiColor, Color, Effects, RgbColor, Style};
use lscolors::{Color as LsColor, FontStyle, Indicator, LsColors, Style as LsStyle};

use crate::entry::{Entry, EntryKind};

/// Resolves the right `anstyle::Style` for each `Entry` from `$LS_COLORS`.
///
/// The lookup classifies the entry into an `LS_COLORS` `Indicator` using only
/// data already on the entry (mode, name, target presence) — no extra stat
/// calls. Extension and filename rules apply to regular files only, matching
/// `ls`.
#[derive(Debug, Clone)]
pub struct Palette {
    inner: LsColors,
}

impl Palette {
    /// Read `$LS_COLORS`, falling back to the GNU `dircolors` defaults when
    /// the variable is unset.
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            inner: LsColors::from_env().unwrap_or_default(),
        }
    }

    #[must_use]
    pub fn from_string(input: &str) -> Self {
        Self {
            inner: LsColors::from_string(input),
        }
    }

    #[must_use]
    pub fn empty() -> Self {
        Self {
            inner: LsColors::empty(),
        }
    }

    #[must_use]
    pub fn style_for(&self, entry: &Entry, target_missing: bool) -> Style {
        let indicator = self.indicator_for(entry, target_missing);
        // Extension/filename rules only fire when classification stayed at
        // `RegularFile`. `file_indicator` only promotes to ex/su/sg/mh when
        // the user has actually configured that indicator, so an unset `ex`
        // lets `*.sh=…` color executable scripts; setting `ex` makes it win.
        // This matches GNU `ls` (`ls.c`: `if (type == C_FILE)` gates the
        // suffix lookup) and the `lscolors` crate.
        //
        // `to_string_lossy` (rather than `to_str`) keeps ASCII suffixes
        // matchable when the rest of the name has invalid UTF-8 bytes —
        // GNU `ls` matches suffixes byte-wise, so the lossy substitution is
        // strictly more permissive than dropping non-UTF-8 names entirely.
        if indicator == Indicator::RegularFile
            && let Some(style) = self.inner.style_for_str(&entry.name.to_string_lossy())
        {
            return to_anstyle(style);
        }
        // `style_for_indicator` has its own fallback chain — e.g. an
        // OrphanedSymbolicLink with no `or` set falls through to `ln`, then
        // `no` — so we don't need to second-guess it here.
        self.inner
            .style_for_indicator(indicator)
            .map_or_else(Style::new, to_anstyle)
    }

    /// Style for a broken symlink's target text (`mi`), if the user has
    /// configured it. Returns `None` when `mi` is unset so the caller can
    /// pick its own visual cue.
    #[must_use]
    pub fn style_for_missing_target(&self) -> Option<Style> {
        self.inner
            .has_explicit_style_for(Indicator::MissingFile)
            .then(|| {
                self.inner
                    .style_for_indicator(Indicator::MissingFile)
                    .map_or_else(Style::new, to_anstyle)
            })
    }

    fn indicator_for(&self, entry: &Entry, target_missing: bool) -> Indicator {
        match entry.kind {
            EntryKind::Directory => self.dir_indicator(entry.mode),
            // `or` colors the link itself; `mi` is for the target string and
            // is left to the caller that renders the target column.
            EntryKind::Symlink if target_missing => Indicator::OrphanedSymbolicLink,
            EntryKind::Symlink => Indicator::SymbolicLink,
            EntryKind::RegularFile => self.file_indicator(entry.mode, entry.nlink),
            EntryKind::CharDevice => Indicator::CharacterDevice,
            EntryKind::BlockDevice => Indicator::BlockDevice,
            EntryKind::Fifo => Indicator::FIFO,
            EntryKind::Socket => Indicator::Socket,
            EntryKind::Other => Indicator::Normal,
        }
    }

    fn dir_indicator(&self, mode: u32) -> Indicator {
        // Mirror lscolors' own precedence: most specific match wins, but only
        // if the user has explicitly configured a style for that indicator —
        // otherwise we'd shadow a more general indicator that *is* set.
        if mode & 0o1002 == 0o1002
            && self
                .inner
                .has_explicit_style_for(Indicator::StickyAndOtherWritable)
        {
            Indicator::StickyAndOtherWritable
        } else if mode & 0o0002 != 0 && self.inner.has_explicit_style_for(Indicator::OtherWritable)
        {
            Indicator::OtherWritable
        } else if mode & 0o1000 != 0 && self.inner.has_explicit_style_for(Indicator::Sticky) {
            Indicator::Sticky
        } else {
            Indicator::Directory
        }
    }

    fn file_indicator(&self, mode: u32, nlink: u64) -> Indicator {
        if mode & 0o4000 != 0 && self.inner.has_explicit_style_for(Indicator::Setuid) {
            Indicator::Setuid
        } else if mode & 0o2000 != 0 && self.inner.has_explicit_style_for(Indicator::Setgid) {
            Indicator::Setgid
        } else if mode & 0o0111 != 0 && self.inner.has_explicit_style_for(Indicator::ExecutableFile)
        {
            Indicator::ExecutableFile
        } else if nlink > 1
            && self
                .inner
                .has_explicit_style_for(Indicator::MultipleHardLinks)
        {
            Indicator::MultipleHardLinks
        } else {
            Indicator::RegularFile
        }
    }
}

fn to_anstyle(s: &LsStyle) -> Style {
    let mut style = Style::new();
    if let Some(fg) = s.foreground {
        style = style.fg_color(Some(to_anstyle_color(fg)));
    }
    if let Some(bg) = s.background {
        style = style.bg_color(Some(to_anstyle_color(bg)));
    }
    style.effects(to_effects(s.font_style))
}

const fn to_anstyle_color(c: LsColor) -> Color {
    match c {
        LsColor::Black => Color::Ansi(AnsiColor::Black),
        LsColor::Red => Color::Ansi(AnsiColor::Red),
        LsColor::Green => Color::Ansi(AnsiColor::Green),
        LsColor::Yellow => Color::Ansi(AnsiColor::Yellow),
        LsColor::Blue => Color::Ansi(AnsiColor::Blue),
        LsColor::Magenta => Color::Ansi(AnsiColor::Magenta),
        LsColor::Cyan => Color::Ansi(AnsiColor::Cyan),
        LsColor::White => Color::Ansi(AnsiColor::White),
        LsColor::BrightBlack => Color::Ansi(AnsiColor::BrightBlack),
        LsColor::BrightRed => Color::Ansi(AnsiColor::BrightRed),
        LsColor::BrightGreen => Color::Ansi(AnsiColor::BrightGreen),
        LsColor::BrightYellow => Color::Ansi(AnsiColor::BrightYellow),
        LsColor::BrightBlue => Color::Ansi(AnsiColor::BrightBlue),
        LsColor::BrightMagenta => Color::Ansi(AnsiColor::BrightMagenta),
        LsColor::BrightCyan => Color::Ansi(AnsiColor::BrightCyan),
        LsColor::BrightWhite => Color::Ansi(AnsiColor::BrightWhite),
        LsColor::Fixed(n) => Color::Ansi256(Ansi256Color(n)),
        LsColor::RGB(r, g, b) => Color::Rgb(RgbColor(r, g, b)),
    }
}

fn to_effects(f: FontStyle) -> Effects {
    let mut e = Effects::new();
    if f.bold {
        e |= Effects::BOLD;
    }
    if f.dimmed {
        e |= Effects::DIMMED;
    }
    if f.italic {
        e |= Effects::ITALIC;
    }
    if f.underline {
        e |= Effects::UNDERLINE;
    }
    if f.slow_blink || f.rapid_blink {
        e |= Effects::BLINK;
    }
    if f.reverse {
        e |= Effects::INVERT;
    }
    if f.hidden {
        e |= Effects::HIDDEN;
    }
    if f.strikethrough {
        e |= Effects::STRIKETHROUGH;
    }
    e
}

#[cfg(test)]
mod tests {
    use super::Palette;
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
            nlink: 1,
            uid: 0,
            gid: 0,
            size: 0,
            rdev: 0,
            mtime: SystemTime::UNIX_EPOCH,
            symlink_target: None,
            symlink_target_is_dir: false,
            dev: 0,
            ino: 0,
        }
    }

    #[test]
    fn gnu_defaults_color_directory_bold_blue() {
        let palette = Palette::from_string("");
        let style = palette.style_for(&entry("d", EntryKind::Directory), false);
        let s = format!("{style}");
        assert!(s.contains("34"), "expected blue SGR: {s:?}");
        assert!(s.contains('1'), "expected bold SGR: {s:?}");
    }

    #[test]
    fn regular_file_has_no_style_under_gnu_defaults() {
        let palette = Palette::from_string("");
        let style = palette.style_for(&entry("f", EntryKind::RegularFile), false);
        assert_eq!(format!("{style}"), String::new());
    }

    #[test]
    fn extension_rule_paints_regular_file() {
        let palette = Palette::from_string("*.rs=38;5;202");
        let style = palette.style_for(&entry("main.rs", EntryKind::RegularFile), false);
        let s = format!("{style}");
        assert!(s.contains("202"), "expected fixed-256 color 202: {s:?}");
    }

    #[test]
    fn executable_indicator_kicks_in_when_set() {
        let palette = Palette::from_string("ex=31");
        let mut e = entry("run", EntryKind::RegularFile);
        e.mode = 0o100_755;
        let style = palette.style_for(&e, false);
        let s = format!("{style}");
        assert!(s.contains("31"), "expected red SGR for ex: {s:?}");
    }

    #[test]
    fn setuid_overrides_executable() {
        let palette = Palette::from_string("ex=31:su=37;41");
        let mut e = entry("priv", EntryKind::RegularFile);
        e.mode = 0o104_755;
        let style = palette.style_for(&e, false);
        let s = format!("{style}");
        assert!(s.contains("41"), "expected red bg for setuid: {s:?}");
    }

    #[test]
    fn other_writable_dir_uses_ow_when_set() {
        let palette = Palette::from_string("ow=30;43");
        let mut e = entry("shared", EntryKind::Directory);
        e.mode = 0o040_777;
        let style = palette.style_for(&e, false);
        let s = format!("{style}");
        assert!(s.contains("43"), "expected yellow bg: {s:?}");
    }

    #[test]
    fn sticky_other_writable_prefers_tw() {
        let palette = Palette::from_string("tw=30;42:ow=30;43");
        let mut e = entry("tmp", EntryKind::Directory);
        e.mode = 0o041_777;
        let style = palette.style_for(&e, false);
        let s = format!("{style}");
        assert!(s.contains("42"), "expected green bg: {s:?}");
    }

    #[test]
    fn orphan_symlink_uses_or_when_set() {
        let palette = Palette::from_string("or=40;31;01");
        let style = palette.style_for(&entry("broken", EntryKind::Symlink), true);
        let s = format!("{style}");
        assert!(s.contains("31"), "expected red SGR: {s:?}");
    }

    #[test]
    fn live_symlink_uses_ln_not_or() {
        let palette = Palette::from_string("ln=35:or=31");
        let style = palette.style_for(&entry("link", EntryKind::Symlink), false);
        let s = format!("{style}");
        assert!(s.contains("35"), "expected magenta SGR: {s:?}");
    }

    #[test]
    fn fifo_socket_devices_map_to_indicators() {
        let palette = Palette::from_string("pi=33:so=01;35:bd=34;46:cd=34;43");
        for (kind, expect) in [
            (EntryKind::Fifo, "33"),
            (EntryKind::Socket, "35"),
            (EntryKind::BlockDevice, "46"),
            (EntryKind::CharDevice, "43"),
        ] {
            let style = palette.style_for(&entry("x", kind), false);
            let s = format!("{style}");
            assert!(s.contains(expect), "{kind:?} expected {expect}: {s:?}");
        }
    }

    #[test]
    fn empty_palette_leaves_everything_unstyled() {
        let palette = Palette::empty();
        for kind in [
            EntryKind::Directory,
            EntryKind::RegularFile,
            EntryKind::Symlink,
            EntryKind::Fifo,
            EntryKind::Socket,
            EntryKind::BlockDevice,
            EntryKind::CharDevice,
            EntryKind::Other,
        ] {
            let style = palette.style_for(&entry("x", kind), false);
            assert_eq!(format!("{style}"), String::new(), "{kind:?}");
        }
    }

    #[test]
    fn non_utf8_name_still_matches_ascii_suffix() {
        use std::os::unix::ffi::OsStringExt;
        let palette = Palette::from_string("*.rs=31");
        let mut e = entry("placeholder", EntryKind::RegularFile);
        // U+FFFD substitution preserves the ".rs" suffix so the extension
        // rule still fires, matching GNU ls's byte-wise suffix matching.
        e.name = OsString::from_vec(vec![b'b', 0xFF, b'.', b'r', b's']);
        let style = palette.style_for(&e, false);
        assert!(format!("{style}").contains("31"));
    }

    #[test]
    fn from_env_does_not_panic() {
        let _ = Palette::from_env();
    }

    #[test]
    fn orphan_symlink_falls_back_to_ln_when_or_unset() {
        // Without `or`, lscolors' own fallback chain hands OrphanedSymbolicLink
        // off to SymbolicLink — a broken link should still get the `ln` color.
        let palette = Palette::from_string("ln=35");
        let style = palette.style_for(&entry("broken", EntryKind::Symlink), true);
        let s = format!("{style}");
        assert!(
            s.contains("35"),
            "or→ln fallback should give magenta: {s:?}"
        );
    }

    #[test]
    fn style_for_missing_target_returns_mi_style_when_set() {
        let palette = Palette::from_string("mi=01;33");
        let style = palette.style_for_missing_target().expect("mi style");
        let s = format!("{style}");
        assert!(s.contains("33"), "expected mi yellow SGR: {s:?}");
    }

    #[test]
    fn style_for_missing_target_returns_none_when_mi_unset() {
        let palette = Palette::from_string("or=31");
        assert!(palette.style_for_missing_target().is_none());
    }

    #[test]
    fn sticky_only_dir_uses_st_indicator() {
        let palette = Palette::from_string("st=37;44");
        let mut e = entry("d", EntryKind::Directory);
        e.mode = 0o041_755; // sticky bit set, world bits unset
        let style = palette.style_for(&e, false);
        let s = format!("{style}");
        assert!(s.contains("44"), "expected blue bg for st: {s:?}");
    }

    #[test]
    fn setgid_indicator_applies_to_sgid_file() {
        let palette = Palette::from_string("sg=30;46");
        let mut e = entry("svc", EntryKind::RegularFile);
        e.mode = 0o102_755; // setgid bit set
        let style = palette.style_for(&e, false);
        let s = format!("{style}");
        assert!(s.contains("46"), "expected cyan bg for sg: {s:?}");
    }

    #[test]
    fn multiple_hard_links_indicator_applies_when_nlink_gt_one() {
        let palette = Palette::from_string("mh=35");
        let mut e = entry("twin", EntryKind::RegularFile);
        e.nlink = 2;
        let style = palette.style_for(&e, false);
        let s = format!("{style}");
        assert!(s.contains("35"), "expected magenta for mh: {s:?}");
    }

    #[test]
    fn bright_colors_convert_through_anstyle() {
        // SGR 90–97 are the bright foreground colors; pair each with a suffix
        // so the conversion path runs for every BrightX variant.
        let palette =
            Palette::from_string("*.bk=90:*.br=91:*.bg=92:*.by=93:*.bu=94:*.bm=95:*.bc=96:*.bw=97");
        for (name, code) in [
            ("a.bk", "90"),
            ("a.br", "91"),
            ("a.bg", "92"),
            ("a.by", "93"),
            ("a.bu", "94"),
            ("a.bm", "95"),
            ("a.bc", "96"),
            ("a.bw", "97"),
        ] {
            let style = palette.style_for(&entry(name, EntryKind::RegularFile), false);
            let s = format!("{style}");
            assert!(s.contains(code), "{name} expected SGR {code}: {s:?}");
        }
    }

    #[test]
    fn rgb_color_converts_through_anstyle() {
        let palette = Palette::from_string("*.rgb=38;2;100;50;25");
        let style = palette.style_for(&entry("a.rgb", EntryKind::RegularFile), false);
        let s = format!("{style}");
        assert!(s.contains("100"), "expected RGB component: {s:?}");
    }

    #[test]
    fn all_font_effects_convert_through_anstyle() {
        // SGR 1;2;3;4;5;7;8;9 = bold;dim;italic;underline;blink;reverse;hidden;strike.
        let palette = Palette::from_string("*.fx=1;2;3;4;5;7;8;9");
        let style = palette.style_for(&entry("a.fx", EntryKind::RegularFile), false);
        let s = format!("{style}");
        for code in ['1', '2', '3', '4', '5', '7', '8', '9'] {
            assert!(s.contains(code), "expected effect SGR {code}: {s:?}");
        }
    }

    #[test]
    fn rapid_blink_also_maps_to_blink_effect() {
        // SGR 6 = rapid_blink, distinct from 5 = slow_blink but both map to
        // anstyle's BLINK effect.
        let palette = Palette::from_string("*.fx=6");
        let style = palette.style_for(&entry("a.fx", EntryKind::RegularFile), false);
        assert!(
            format!("{style}").contains('5'),
            "rapid_blink → anstyle BLINK (SGR 5)"
        );
    }
}
