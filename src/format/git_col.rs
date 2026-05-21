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

use anstyle::{AnsiColor, Color, Effects, Style};

use crate::git::PorcelainCode;

pub const WIDTH: usize = 2;

#[must_use]
pub fn render(code: PorcelainCode) -> String {
    let style = style_for(code);
    format!(
        "{style}{}{}{}",
        code.index,
        code.worktree,
        style.render_reset()
    )
}

const fn style_for(code: PorcelainCode) -> Style {
    match code {
        PorcelainCode::CLEAN | PorcelainCode::IGNORED => Style::new().effects(Effects::DIMMED),
        PorcelainCode::UNTRACKED => Style::new().fg_color(Some(Color::Ansi(AnsiColor::Magenta))),
        _ => Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow))),
    }
}

#[cfg(test)]
mod tests {
    use super::render;
    use crate::git::PorcelainCode;

    #[test]
    fn render_includes_both_chars() {
        for code in [
            PorcelainCode::CLEAN,
            PorcelainCode::UNTRACKED,
            PorcelainCode::IGNORED,
            PorcelainCode::MODIFIED_WORKTREE,
            PorcelainCode::DELETED_WORKTREE,
            PorcelainCode::TYPE_CHANGE_WORKTREE,
            PorcelainCode::RENAMED,
            PorcelainCode::COPIED,
            PorcelainCode::UNMERGED,
            PorcelainCode::DIRTY_SUBTREE,
        ] {
            let s = render(code);
            assert!(s.contains(code.index));
            assert!(s.contains(code.worktree));
        }
    }

    #[test]
    fn clean_and_ignored_are_dimmed() {
        for code in [PorcelainCode::CLEAN, PorcelainCode::IGNORED] {
            let s = render(code);
            assert!(s.contains("\x1b[2m"));
        }
    }

    #[test]
    fn untracked_is_magenta() {
        let s = render(PorcelainCode::UNTRACKED);
        assert!(s.contains("\x1b[35m"));
    }

    #[test]
    fn modifications_are_yellow() {
        let s = render(PorcelainCode::MODIFIED_WORKTREE);
        assert!(s.contains("\x1b[33m"));
    }

    #[test]
    fn dirty_subtree_is_yellow_asterisk() {
        let s = render(PorcelainCode::DIRTY_SUBTREE);
        assert!(s.contains('*'));
        assert!(s.contains("\x1b[33m"));
        // Must NOT be dimmed — that's reserved for CLEAN/IGNORED.
        assert!(!s.contains("\x1b[2m"));
    }
}
