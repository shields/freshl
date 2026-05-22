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

use anstyle::{AnsiColor, Color, Style};

use crate::git::PorcelainCode;

pub const WIDTH: usize = 1;

#[must_use]
pub fn render(code: PorcelainCode) -> String {
    let style = style_for(code);
    format!("{style}{}{}", code.glyph(), style.render_reset())
}

const fn style_for(code: PorcelainCode) -> Style {
    match code {
        PorcelainCode::CLEAN | PorcelainCode::IGNORED => return Style::new().dimmed(),
        PorcelainCode::UNTRACKED => {
            return Style::new().fg_color(Some(Color::Ansi(AnsiColor::Magenta)));
        }
        PorcelainCode::UNMERGED => {
            return Style::new()
                .fg_color(Some(Color::Ansi(AnsiColor::Red)))
                .bold();
        }
        PorcelainCode::DIRTY_SUBTREE => {
            return Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));
        }
        _ => {}
    }
    // Mutable states (single-column or merged combinations). Hue comes from
    // the rendered glyph, derived from named constants where possible. The
    // only literal glyph below is `+`: addition has no PorcelainCode
    // constant — it only exists as an idx_char set in handle_tree_index.
    let glyph = code.glyph();
    let hue = if glyph == PorcelainCode::DELETED_WORKTREE.worktree {
        AnsiColor::Red
    } else if glyph == '+' {
        AnsiColor::Green
    } else {
        AnsiColor::Cyan
    };
    let style = Style::new().fg_color(Some(Color::Ansi(hue)));
    if code.index == ' ' {
        style
    } else {
        style.bold()
    }
}

#[cfg(test)]
mod tests {
    use anstyle::{AnsiColor, Color, Effects};

    use super::{render, style_for};
    use crate::git::PorcelainCode;

    fn hue_of(code: PorcelainCode) -> Option<Color> {
        style_for(code).get_fg_color()
    }

    fn is_bold(code: PorcelainCode) -> bool {
        style_for(code).get_effects().contains(Effects::BOLD)
    }

    fn is_dimmed(code: PorcelainCode) -> bool {
        style_for(code).get_effects().contains(Effects::DIMMED)
    }

    #[test]
    fn glyph_picks_worktree_when_set() {
        assert_eq!(PorcelainCode::MODIFIED_WORKTREE.glyph(), '●');
    }

    #[test]
    fn glyph_falls_back_to_index_when_worktree_blank() {
        assert_eq!(PorcelainCode::RENAMED.glyph(), '→');
        assert_eq!(PorcelainCode::DIRTY_SUBTREE.glyph(), '⋯');
        assert_eq!(PorcelainCode::BLANK.with_index('+').glyph(), '+');
    }

    #[test]
    fn render_wraps_glyph_in_ansi() {
        let s = render(PorcelainCode::MODIFIED_WORKTREE);
        assert!(s.contains('●'));
        assert!(s.starts_with("\x1b["));
        assert!(s.ends_with("\x1b[0m"));
    }

    #[test]
    fn clean_and_ignored_are_dimmed() {
        assert!(is_dimmed(PorcelainCode::CLEAN));
        assert!(is_dimmed(PorcelainCode::IGNORED));
    }

    #[test]
    fn untracked_is_magenta() {
        assert_eq!(
            hue_of(PorcelainCode::UNTRACKED),
            Some(Color::Ansi(AnsiColor::Magenta))
        );
    }

    #[test]
    fn addition_is_bold_green() {
        let staged_add = PorcelainCode::BLANK.with_index('+');
        assert_eq!(hue_of(staged_add), Some(Color::Ansi(AnsiColor::Green)));
        assert!(is_bold(staged_add));
    }

    #[test]
    fn modification_is_cyan() {
        assert_eq!(
            hue_of(PorcelainCode::MODIFIED_WORKTREE),
            Some(Color::Ansi(AnsiColor::Cyan))
        );
    }

    #[test]
    fn deletion_is_red() {
        assert_eq!(
            hue_of(PorcelainCode::DELETED_WORKTREE),
            Some(Color::Ansi(AnsiColor::Red))
        );
    }

    #[test]
    fn type_change_is_cyan() {
        assert_eq!(
            hue_of(PorcelainCode::TYPE_CHANGE_WORKTREE),
            Some(Color::Ansi(AnsiColor::Cyan))
        );
    }

    #[test]
    fn rename_is_cyan() {
        assert_eq!(
            hue_of(PorcelainCode::RENAMED_WORKTREE),
            Some(Color::Ansi(AnsiColor::Cyan))
        );
    }

    #[test]
    fn copy_is_cyan() {
        assert_eq!(
            hue_of(PorcelainCode::COPIED_WORKTREE),
            Some(Color::Ansi(AnsiColor::Cyan))
        );
    }

    #[test]
    fn staged_change_is_bold() {
        assert!(is_bold(PorcelainCode::RENAMED));
    }

    #[test]
    fn worktree_only_change_is_not_bold() {
        assert!(!is_bold(PorcelainCode::MODIFIED_WORKTREE));
    }

    #[test]
    fn staged_and_worktree_modify_is_bold_cyan() {
        let combo = PorcelainCode::MODIFIED_WORKTREE.with_index('●');
        assert_eq!(hue_of(combo), Some(Color::Ansi(AnsiColor::Cyan)));
        assert!(is_bold(combo));
    }

    #[test]
    fn dirty_subtree_is_cyan() {
        assert_eq!(
            hue_of(PorcelainCode::DIRTY_SUBTREE),
            Some(Color::Ansi(AnsiColor::Cyan))
        );
        assert!(!is_dimmed(PorcelainCode::DIRTY_SUBTREE));
    }

    #[test]
    fn conflict_is_red_bold() {
        assert_eq!(
            hue_of(PorcelainCode::UNMERGED),
            Some(Color::Ansi(AnsiColor::Red))
        );
        assert!(is_bold(PorcelainCode::UNMERGED));
    }
}
