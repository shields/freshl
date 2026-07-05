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

use anstyle::{Ansi256Color, Color, Style};

const DARK_BACKGROUND_GRAY: u8 = 243;
const LIGHT_BACKGROUND_GRAY: u8 = 249;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Background {
    Dark,
    Light,
}

#[must_use]
pub fn from_env() -> Style {
    style_for_colorfgbg(std::env::var("COLORFGBG").ok().as_deref())
}

fn style_for_colorfgbg(value: Option<&str>) -> Style {
    value
        .and_then(style_from_value)
        .unwrap_or_else(fallback_dim)
}

fn style_from_value(value: &str) -> Option<Style> {
    let bg = value.split(';').next_back()?.parse::<u8>().ok()?;
    Some(match background(bg)? {
        Background::Dark => gray(DARK_BACKGROUND_GRAY),
        Background::Light => gray(LIGHT_BACKGROUND_GRAY),
    })
}

const fn background(index: u8) -> Option<Background> {
    match index {
        0..=6 | 8 => Some(Background::Dark),
        7 | 9..=15 => Some(Background::Light),
        _ => None,
    }
}

const fn gray(index: u8) -> Style {
    Style::new().fg_color(Some(Color::Ansi256(Ansi256Color(index))))
}

const fn fallback_dim() -> Style {
    Style::new().dimmed()
}

#[cfg(test)]
mod tests {
    use anstyle::{Ansi256Color, Color, Effects};

    use super::{
        DARK_BACKGROUND_GRAY, LIGHT_BACKGROUND_GRAY, background, fallback_dim, gray,
        style_for_colorfgbg,
    };

    #[test]
    fn colorfgbg_dark_background_selects_dark_gray() {
        assert_eq!(
            style_for_colorfgbg(Some("15;0")),
            gray(DARK_BACKGROUND_GRAY)
        );
    }

    #[test]
    fn colorfgbg_light_background_selects_light_gray() {
        assert_eq!(
            style_for_colorfgbg(Some("0;15")),
            gray(LIGHT_BACKGROUND_GRAY)
        );
    }

    #[test]
    fn missing_or_malformed_colorfgbg_falls_back_to_sgr_dim() {
        for value in [None, Some(""), Some("0;not-a-number"), Some("0;")] {
            let style = style_for_colorfgbg(value);
            assert_eq!(style, fallback_dim());
            assert!(style.get_effects().contains(Effects::DIMMED));
        }
    }

    #[test]
    fn ambiguous_colorfgbg_background_falls_back_to_sgr_dim() {
        assert_eq!(style_for_colorfgbg(Some("0;16")), fallback_dim());
    }

    #[test]
    fn representative_dark_indexes_classify_as_dark() {
        for index in [0, 1, 2, 3, 4, 5, 6, 8] {
            assert_eq!(background(index), Some(super::Background::Dark));
        }
    }

    #[test]
    fn representative_light_indexes_classify_as_light() {
        for index in [7, 9, 10, 11, 12, 13, 14, 15] {
            assert_eq!(background(index), Some(super::Background::Light));
        }
    }

    #[test]
    fn gray_styles_use_ansi_256_foregrounds() {
        assert_eq!(
            gray(DARK_BACKGROUND_GRAY).get_fg_color(),
            Some(Color::Ansi256(Ansi256Color(DARK_BACKGROUND_GRAY)))
        );
        assert_eq!(
            gray(LIGHT_BACKGROUND_GRAY).get_fg_color(),
            Some(Color::Ansi256(Ansi256Color(LIGHT_BACKGROUND_GRAY)))
        );
    }
}
