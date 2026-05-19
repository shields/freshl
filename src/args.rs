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

use std::ffi::OsString;
use std::path::PathBuf;

use crate::sort::SortKey;

// Each bool toggles an independent CLI flag, so collapsing them into an enum
// would obscure the parser more than it would clarify the type.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ListOptions {
    pub recursive: bool,
    pub sort_key: SortKey,
    pub reverse: bool,
    /// Ripgrep-style escalation gate for `-R` descent.
    ///   0 — skip hidden and gitignored directories (default)
    ///   1 — skip hidden only
    ///   2 — descend into everything
    /// Saturates at 2 (a third `u` is silently treated as 2).
    pub unrestricted: u8,
    /// `-d`: render each path as a single row instead of expanding
    /// directories. Suppresses `-R` for the same reason GNU `ls` does
    /// (the descent is the thing `-d` opts out of).
    pub directory: bool,
    /// `-L`: dereference symlinks; reclassify as the target's kind so
    /// symlinks-to-dirs expand under `-R`.
    pub follow_symlinks: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    List {
        paths: Vec<PathBuf>,
        options: ListOptions,
    },
    Help,
    Version,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ArgError {
    pub message: String,
}

pub const HELP: &str = "\
Usage: freshl [OPTION]... [PATH]...

A modern replacement for `ls`. One opinionated layout: type, mode, links,
owner, group, size (raw bytes grouped in clusters of six), mtime as ISO 8601
UTC, optional git status, name. Hidden files are always listed.

Options:
  -R         Recurse into directories (depth-first).
  -S         Sort by size, largest first.
  -t         Sort by mtime, newest first.
  -r         Reverse the resulting order. Directories still group first.
  -u         Recurse into gitignored directories (with -R). Repeat (-uu) to
             also recurse into hidden directories.
  -d         List directories themselves, not their contents. Suppresses -R.
  -L         Dereference symlinks: show the target's metadata, not the link's.
  -h, --help     Print this help.
      --version  Print version.
      --        Treat following arguments as paths.

Short flags may be bundled (e.g. -Rt, -Sr, -Ruu).
";

#[must_use]
pub fn version_line() -> String {
    format!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
}

/// Parse command-line arguments into an [`Action`].
///
/// # Errors
///
/// Returns [`ArgError`] if an unrecognised flag is supplied.
pub fn parse<I>(raw: I) -> Result<Action, ArgError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut options = ListOptions::default();
    let mut positional_only = false;

    for arg in raw {
        if positional_only {
            paths.push(PathBuf::from(arg));
            continue;
        }
        let bytes = arg.as_encoded_bytes();
        if bytes == b"--" {
            positional_only = true;
        } else if bytes == b"--help" {
            return Ok(Action::Help);
        } else if bytes == b"--version" {
            return Ok(Action::Version);
        } else if bytes.len() >= 2 && bytes[0] == b'-' && bytes[1] != b'-' {
            // Short-flag cluster. `-h` anywhere short-circuits to help; check
            // first so we don't mutate `options` only to throw the result away.
            if bytes[1..].contains(&b'h') {
                return Ok(Action::Help);
            }
            for &b in &bytes[1..] {
                match b {
                    b'R' => options.recursive = true,
                    b'S' => options.sort_key = SortKey::Size,
                    b't' => options.sort_key = SortKey::Time,
                    b'r' => options.reverse = true,
                    b'u' => options.unrestricted = options.unrestricted.saturating_add(1).min(2),
                    b'd' => options.directory = true,
                    b'L' => options.follow_symlinks = true,
                    _ => {
                        return Err(ArgError {
                            message: format!("unknown option: {}", arg.to_string_lossy()),
                        });
                    }
                }
            }
        } else if bytes.starts_with(b"-") && bytes != b"-" {
            // Long flag that wasn't matched above (e.g. `--bogus`).
            return Err(ArgError {
                message: format!("unknown option: {}", arg.to_string_lossy()),
            });
        } else {
            paths.push(PathBuf::from(arg));
        }
    }

    Ok(Action::List { paths, options })
}

#[cfg(test)]
mod tests {
    use super::{Action, ListOptions, parse, version_line};
    use crate::sort::SortKey;
    use std::ffi::OsString;
    use std::path::PathBuf;

    fn args(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    fn list(paths: Vec<PathBuf>, options: ListOptions) -> Action {
        Action::List { paths, options }
    }

    #[test]
    fn empty_means_list_with_no_paths() {
        assert_eq!(parse(args(&[])), Ok(list(vec![], ListOptions::default())));
    }

    #[test]
    fn single_positional_path() {
        assert_eq!(
            parse(args(&["src"])),
            Ok(list(vec![PathBuf::from("src")], ListOptions::default()))
        );
    }

    #[test]
    fn multiple_positional_paths() {
        assert_eq!(
            parse(args(&["a", "b", "c"])),
            Ok(list(
                vec![PathBuf::from("a"), PathBuf::from("b"), PathBuf::from("c"),],
                ListOptions::default()
            ))
        );
    }

    #[test]
    fn help_short_flag() {
        assert_eq!(parse(args(&["-h"])), Ok(Action::Help));
    }

    #[test]
    fn help_long_flag() {
        assert_eq!(parse(args(&["--help", "ignored"])), Ok(Action::Help));
    }

    #[test]
    fn version_long_flag() {
        assert_eq!(parse(args(&["--version", "ignored"])), Ok(Action::Version));
    }

    #[test]
    fn double_dash_treats_following_as_paths() {
        assert_eq!(
            parse(args(&["--", "--help", "-foo"])),
            Ok(list(
                vec![PathBuf::from("--help"), PathBuf::from("-foo"),],
                ListOptions::default()
            ))
        );
    }

    #[test]
    fn unknown_long_flag_errors() {
        let err = parse(args(&["--what"])).unwrap_err();
        assert!(err.message.contains("--what"));
    }

    #[test]
    fn single_dash_is_a_path() {
        assert_eq!(
            parse(args(&["-"])),
            Ok(list(vec![PathBuf::from("-")], ListOptions::default()))
        );
    }

    #[test]
    fn version_line_includes_name_and_version() {
        let v = version_line();
        assert!(v.starts_with("freshl "));
        assert!(v.len() > "freshl ".len());
    }

    #[test]
    fn help_text_mentions_usage() {
        assert!(super::HELP.contains("Usage: freshl"));
    }

    fn list_with(opts: ListOptions) -> Action {
        Action::List {
            paths: vec![],
            options: opts,
        }
    }

    #[test]
    fn recursive_short_flag_sets_recursive() {
        assert_eq!(
            parse(args(&["-R"])),
            Ok(list_with(ListOptions {
                recursive: true,
                ..ListOptions::default()
            }))
        );
    }

    #[test]
    fn size_short_flag_sets_sort_key() {
        assert_eq!(
            parse(args(&["-S"])),
            Ok(list_with(ListOptions {
                sort_key: SortKey::Size,
                ..ListOptions::default()
            }))
        );
    }

    #[test]
    fn time_short_flag_sets_sort_key() {
        assert_eq!(
            parse(args(&["-t"])),
            Ok(list_with(ListOptions {
                sort_key: SortKey::Time,
                ..ListOptions::default()
            }))
        );
    }

    #[test]
    fn reverse_short_flag_sets_reverse() {
        assert_eq!(
            parse(args(&["-r"])),
            Ok(list_with(ListOptions {
                reverse: true,
                ..ListOptions::default()
            }))
        );
    }

    #[test]
    fn bundled_cluster_sets_multiple_flags() {
        assert_eq!(
            parse(args(&["-Rt"])),
            Ok(list_with(ListOptions {
                recursive: true,
                sort_key: SortKey::Time,
                ..ListOptions::default()
            }))
        );
    }

    #[test]
    fn bundled_cluster_with_reverse_size_and_recursive() {
        assert_eq!(
            parse(args(&["-rSR"])),
            Ok(list_with(ListOptions {
                recursive: true,
                sort_key: SortKey::Size,
                reverse: true,
                ..ListOptions::default()
            }))
        );
    }

    #[test]
    fn unknown_letter_in_cluster_errors_with_cluster_in_message() {
        let err = parse(args(&["-RX"])).unwrap_err();
        assert!(err.message.contains("-RX"), "got: {}", err.message);
    }

    #[test]
    fn single_u_sets_unrestricted_to_one() {
        assert_eq!(
            parse(args(&["-u"])),
            Ok(list_with(ListOptions {
                unrestricted: 1,
                ..ListOptions::default()
            }))
        );
    }

    #[test]
    fn double_u_sets_unrestricted_to_two() {
        assert_eq!(
            parse(args(&["-uu"])),
            Ok(list_with(ListOptions {
                unrestricted: 2,
                ..ListOptions::default()
            }))
        );
    }

    #[test]
    fn triple_u_saturates_at_two() {
        assert_eq!(
            parse(args(&["-uuu"])),
            Ok(list_with(ListOptions {
                unrestricted: 2,
                ..ListOptions::default()
            }))
        );
    }

    #[test]
    fn separate_u_flags_accumulate_to_two() {
        assert_eq!(
            parse(args(&["-u", "-u"])),
            Ok(list_with(ListOptions {
                unrestricted: 2,
                ..ListOptions::default()
            }))
        );
    }

    #[test]
    fn h_in_cluster_short_circuits_to_help() {
        assert_eq!(parse(args(&["-Rh"])), Ok(Action::Help));
    }

    #[test]
    fn directory_short_flag_sets_directory() {
        assert_eq!(
            parse(args(&["-d"])),
            Ok(list_with(ListOptions {
                directory: true,
                ..ListOptions::default()
            }))
        );
    }

    #[test]
    fn directory_bundles_with_other_flags() {
        assert_eq!(
            parse(args(&["-dR"])),
            Ok(list_with(ListOptions {
                directory: true,
                recursive: true,
                ..ListOptions::default()
            }))
        );
    }

    #[test]
    fn follow_short_flag_sets_follow_symlinks() {
        assert_eq!(
            parse(args(&["-L"])),
            Ok(list_with(ListOptions {
                follow_symlinks: true,
                ..ListOptions::default()
            }))
        );
    }

    #[test]
    fn follow_bundles_with_recursive() {
        assert_eq!(
            parse(args(&["-LR"])),
            Ok(list_with(ListOptions {
                follow_symlinks: true,
                recursive: true,
                ..ListOptions::default()
            }))
        );
    }

    #[test]
    fn paths_after_flags_still_collected() {
        assert_eq!(
            parse(args(&["-R", "src", "docs"])).unwrap(),
            Action::List {
                paths: vec![PathBuf::from("src"), PathBuf::from("docs")],
                options: ListOptions {
                    recursive: true,
                    ..ListOptions::default()
                },
            }
        );
    }
}
