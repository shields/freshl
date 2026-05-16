use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    List(Vec<PathBuf>),
    Help,
    Version,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ArgError {
    pub message: String,
}

pub const HELP: &str = "\
Usage: freshl [PATH]...

A modern replacement for `ls`. One opinionated layout: type, mode, links,
owner, group, size (raw bytes grouped in clusters of six), mtime as ISO 8601
UTC, optional git status, name. Hidden files are always listed.

Options:
  -h, --help     Print this help.
      --version  Print version.
      --        Treat following arguments as paths.
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
    let mut positional_only = false;

    for arg in raw {
        if positional_only {
            paths.push(PathBuf::from(arg));
            continue;
        }
        let bytes = arg.as_encoded_bytes();
        if bytes == b"--" {
            positional_only = true;
        } else if bytes == b"--help" || bytes == b"-h" {
            return Ok(Action::Help);
        } else if bytes == b"--version" {
            return Ok(Action::Version);
        } else if bytes.starts_with(b"-") && bytes != b"-" {
            return Err(ArgError {
                message: format!("unknown option: {}", arg.to_string_lossy()),
            });
        } else {
            paths.push(PathBuf::from(arg));
        }
    }

    Ok(Action::List(paths))
}

#[cfg(test)]
mod tests {
    use super::{Action, parse, version_line};
    use std::ffi::OsString;
    use std::path::PathBuf;

    fn args(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    #[test]
    fn empty_means_list_with_no_paths() {
        assert_eq!(parse(args(&[])), Ok(Action::List(vec![])));
    }

    #[test]
    fn single_positional_path() {
        assert_eq!(
            parse(args(&["src"])),
            Ok(Action::List(vec![PathBuf::from("src")]))
        );
    }

    #[test]
    fn multiple_positional_paths() {
        assert_eq!(
            parse(args(&["a", "b", "c"])),
            Ok(Action::List(vec![
                PathBuf::from("a"),
                PathBuf::from("b"),
                PathBuf::from("c"),
            ]))
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
            Ok(Action::List(vec![
                PathBuf::from("--help"),
                PathBuf::from("-foo"),
            ]))
        );
    }

    #[test]
    fn unknown_flag_errors() {
        let err = parse(args(&["--what"])).unwrap_err();
        assert!(err.message.contains("--what"));
    }

    #[test]
    fn single_dash_is_a_path() {
        assert_eq!(
            parse(args(&["-"])),
            Ok(Action::List(vec![PathBuf::from("-")]))
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
}
