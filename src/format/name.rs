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

use std::io::Write;
use std::os::unix::ffi::OsStrExt;

use anstyle::{AnsiColor, Color, Effects, Style};

use crate::entry::Entry;
use crate::format::palette::Palette;

/// Render the styled name as raw bytes.
///
/// ANSI escape sequences are interleaved with the filename's underlying bytes
/// verbatim, so non-UTF-8 names round-trip exactly to a pipe while still
/// rendering with the right colour on a terminal. Symlinks render as
/// `name → target`, walking the full chain on multi-hop links. The
/// link/intermediate names render in the `ln` symlink color, the arrows
/// dim, and the final target in its natural per-kind color. A broken link
/// walks the same chain up to the break — the link side renders in the orphan
/// (`or`) color and the unresolved final hop in `mi` (or red when `mi` is
/// unset).
#[must_use]
pub fn format_name(palette: &Palette, entry: &Entry, dim_if_ignored: bool) -> Vec<u8> {
    let base = palette.style_for(entry);
    let dim = Style::new().effects(Effects::DIMMED);
    let red = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Red)));
    let overlay = |s: Style| {
        if dim_if_ignored {
            // OR DIMMED onto whatever the palette already set (bold, fg, …)
            // so the dim cue doesn't overwrite the type/extension styling.
            s.effects(s.get_effects() | Effects::DIMMED)
        } else {
            s
        }
    };
    let name_style = overlay(base);

    let chain_bytes: usize = entry
        .follow_chain
        .iter()
        .map(|p| p.as_os_str().len() + 16)
        .sum();
    let mut out = Vec::with_capacity(entry.name.len() + 32 + chain_bytes);

    let segment = |out: &mut Vec<u8>, style: Style, bytes: &[u8]| {
        let _ = write!(out, "{style}");
        out.extend_from_slice(bytes);
        let _ = write!(out, "{}", style.render_reset());
    };

    if let Some((final_target, hops)) = entry.follow_chain.split_last() {
        // One renderer for both resolved and broken links. A resolved link is
        // reclassified to its target's kind, so `name_style` is the target's
        // per-kind style and the link side (name + intermediates) takes `ln`.
        // A broken link keeps its `Symlink` kind, so `name_style` is the orphan
        // (`or`) style for the link side and the unresolved final hop takes
        // `mi` (or red when `mi` is unset).
        let broken = entry.is_broken_link();
        let link_style = if broken {
            name_style
        } else {
            overlay(palette.style_for_symlink())
        };
        let final_style = if broken {
            overlay(palette.style_for_missing_target().unwrap_or(red))
        } else {
            name_style
        };
        segment(&mut out, link_style, entry.name.as_bytes());
        for intermediate in hops {
            segment(&mut out, dim, " → ".as_bytes());
            segment(&mut out, link_style, intermediate.as_os_str().as_bytes());
        }
        segment(&mut out, dim, " → ".as_bytes());
        segment(&mut out, final_style, final_target.as_os_str().as_bytes());
        return out;
    }

    // No chain: a plain file/dir, or the rare broken link whose `readlink`
    // itself failed (rendered as the bare name in the orphan style).
    segment(&mut out, name_style, entry.name.as_bytes());
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
            dev: 0,
            ino: 0,
            follow_chain: Vec::new(),
        }
    }

    fn as_lossy(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    #[test]
    fn formats_plain_file_name() {
        let palette = Palette::empty();
        let e = entry("hello", EntryKind::RegularFile);
        let bytes = format_name(&palette, &e, false);
        assert!(as_lossy(&bytes).contains("hello"));
    }

    #[test]
    fn broken_symlink_renders_arrow_with_red_target_when_mi_unset() {
        let palette = Palette::empty();
        let mut e = entry("link", EntryKind::Symlink);
        e.follow_chain = vec![PathBuf::from("nowhere")];
        let s = as_lossy(&format_name(&palette, &e, false));
        assert!(s.contains("link"));
        assert!(s.contains('→'));
        assert!(s.contains("nowhere"));
        // SGR 31 is anstyle's red; without `mi` configured the target falls
        // back to freshl's hardcoded red.
        assert!(s.contains("31"), "expected red SGR for missing target: {s}");
    }

    #[test]
    fn broken_symlink_target_uses_mi_when_palette_sets_it() {
        let palette = Palette::from_string("mi=01;33");
        let mut e = entry("link", EntryKind::Symlink);
        e.follow_chain = vec![PathBuf::from("nowhere")];
        let s = as_lossy(&format_name(&palette, &e, false));
        assert!(s.contains("33"), "expected mi yellow on target: {s}");
        assert!(!s.contains("31"), "freshl red should not appear: {s}");
    }

    #[test]
    fn broken_multi_hop_chain_renders_full_path_with_red_tail() {
        // a → mid → gone, with `gone` missing: the whole chain renders and only
        // the unresolved tail takes red (mi unset here).
        let palette = Palette::empty();
        let mut e = entry("a", EntryKind::Symlink);
        e.follow_chain = vec![PathBuf::from("mid"), PathBuf::from("gone")];
        let s = as_lossy(&format_name(&palette, &e, false));
        let a = s.find('a').expect("name missing");
        let mid = s.find("mid").expect("intermediate missing");
        let gone = s.find("gone").expect("tail missing");
        assert!(
            a < mid && mid < gone,
            "expected forward order a → mid → gone: {s}"
        );
        assert_eq!(s.matches('→').count(), 2, "two arrows for two hops: {s}");
        assert!(s.contains("31"), "unresolved tail should be red: {s}");
    }

    #[test]
    fn ignored_files_get_dim_style() {
        let palette = Palette::empty();
        let e = entry("ignored", EntryKind::RegularFile);
        let dim = format_name(&palette, &e, true);
        let plain = format_name(&palette, &e, false);
        assert_ne!(plain, dim);
    }

    #[test]
    fn ignored_dim_preserves_palette_styling() {
        // With a non-empty palette, the DIMMED overlay must layer onto the
        // base style instead of replacing it.
        let palette = Palette::from_string("di=01;34");
        let e = entry("d", EntryKind::Directory);
        let dim = as_lossy(&format_name(&palette, &e, true));
        assert!(
            dim.contains("34"),
            "blue fg should survive dim overlay: {dim}"
        );
        assert!(dim.contains('2'), "dim effect should be present: {dim}");
    }

    #[test]
    fn formats_follow_chain_with_arrows_forward_to_target() {
        let palette = Palette::empty();
        let mut e = entry("CLAUDE.md", EntryKind::RegularFile);
        e.follow_chain = vec![PathBuf::from("AGENTS.md")];
        let s = as_lossy(&format_name(&palette, &e, false));
        // Each segment carries its own style with explicit resets, so the
        // arrow and target are sandwiched between ANSI control sequences
        // rather than being a contiguous substring. Spot-check order.
        let name_pos = s.find("CLAUDE.md").expect("name missing");
        let target_pos = s.find("AGENTS.md").expect("target missing");
        assert!(name_pos < target_pos, "name must precede target: {s}");
        assert!(s.contains('→'), "forward arrow missing: {s}");
        assert!(!s.contains("<-"), "no reverse arrow: {s}");
    }

    #[test]
    fn formats_multi_hop_follow_chain_in_forward_order() {
        let palette = Palette::empty();
        let mut e = entry("top", EntryKind::RegularFile);
        e.follow_chain = vec![PathBuf::from("mid"), PathBuf::from("target")];
        let s = as_lossy(&format_name(&palette, &e, false));
        let prefix_pos = s.find("top").expect("name missing");
        let mid_pos = s.find("mid").expect("intermediate missing");
        let target_pos = s.find("target").expect("target missing");
        assert!(
            prefix_pos < mid_pos && mid_pos < target_pos,
            "expected forward order name → mid → target: {s}"
        );
    }

    #[test]
    fn follow_chain_left_side_uses_symlink_color() {
        // `ln=01;36` is the `LS_COLORS` default for symlinks: bold cyan.
        // The link name and any intermediates should render with that SGR
        // 36, not with the dim-only fallback.
        let palette = Palette::from_string("ln=01;36");
        let mut e = entry("CLAUDE.md", EntryKind::RegularFile);
        e.follow_chain = vec![PathBuf::from("AGENTS.md")];
        let s = as_lossy(&format_name(&palette, &e, false));
        assert!(s.contains("36"), "symlink cyan SGR missing: {s}");
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
            dev: 0,
            ino: 0,
            follow_chain: Vec::new(),
        };
        let bytes = format_name(&palette, &e, false);
        // The raw byte 0xFF must appear in the output verbatim, not as U+FFFD.
        assert!(bytes.windows(raw.len()).any(|w| w == raw.as_slice()));
    }
}
