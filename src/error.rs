use std::fmt;
use std::process::ExitCode;

#[derive(Debug)]
pub enum Error {
    Usage(String),
    Io {
        path: String,
        source: std::io::Error,
    },
}

impl Error {
    #[must_use]
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::Usage(_) => ExitCode::from(2),
            Self::Io { .. } => ExitCode::from(1),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(msg) => write!(f, "freshl: {msg}"),
            Self::Io { path, source } => write!(f, "freshl: {path}: {source}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Error;
    use std::process::ExitCode;

    fn code_byte(code: ExitCode) -> String {
        format!("{code:?}")
    }

    #[test]
    fn usage_uses_exit_code_two() {
        let err = Error::Usage("nope".into());
        assert_eq!(code_byte(err.exit_code()), code_byte(ExitCode::from(2)));
        assert_eq!(format!("{err}"), "freshl: nope");
    }

    #[test]
    fn io_uses_exit_code_one() {
        let err = Error::Io {
            path: "thing".into(),
            source: std::io::Error::other("boom"),
        };
        assert_eq!(code_byte(err.exit_code()), code_byte(ExitCode::from(1)));
        let rendered = format!("{err}");
        assert!(rendered.contains("thing"));
        assert!(rendered.contains("boom"));
    }
}
