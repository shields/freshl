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
}
