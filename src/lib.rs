use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;

pub mod args;
pub mod error;

use args::{Action, parse};
use error::Error;

#[must_use]
pub fn run<I>(raw: I, stdout: &mut dyn Write, stderr: &mut dyn Write) -> ExitCode
where
    I: IntoIterator<Item = OsString>,
{
    match dispatch(raw, stdout) {
        Ok(code) => code,
        Err(err) => {
            let _ = writeln!(stderr, "{err}");
            err.exit_code()
        }
    }
}

fn dispatch<I>(raw: I, stdout: &mut dyn Write) -> Result<ExitCode, Error>
where
    I: IntoIterator<Item = OsString>,
{
    let action = parse(raw).map_err(|e| Error::Usage(e.message))?;
    match action {
        Action::Help => write_stdout(stdout, args::HELP.as_bytes())?,
        Action::Version => write_stdout(stdout, format!("{}\n", args::version_line()).as_bytes())?,
        Action::List(_) => {}
    }
    Ok(ExitCode::SUCCESS)
}

fn write_stdout(stdout: &mut dyn Write, bytes: &[u8]) -> Result<(), Error> {
    stdout.write_all(bytes).map_err(|source| Error::Io {
        path: "<stdout>".into(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::run;
    use std::ffi::OsString;
    use std::io::{self, Write};

    fn os(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    fn code_repr(code: std::process::ExitCode) -> String {
        format!("{code:?}")
    }

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("nope"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn failing_writer_flush_is_a_noop() {
        assert!(FailingWriter.flush().is_ok());
    }

    #[test]
    fn help_writes_to_stdout_and_returns_success() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(os(&["--help"]), &mut out, &mut err);
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Usage: freshl"));
        assert!(err.is_empty());
    }

    #[test]
    fn version_writes_to_stdout() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(os(&["--version"]), &mut out, &mut err);
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("freshl "));
    }

    #[test]
    fn unknown_flag_writes_to_stderr_and_returns_two() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(os(&["--bogus"]), &mut out, &mut err);
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::from(2)));
        assert!(out.is_empty());
        let text = String::from_utf8(err).unwrap();
        assert!(text.contains("--bogus"));
    }

    #[test]
    fn list_action_is_currently_a_no_op() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(os(&["some-path"]), &mut out, &mut err);
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
        assert!(out.is_empty());
        assert!(err.is_empty());
    }

    #[test]
    fn stdout_write_failure_surfaces_io_error() {
        let mut out = FailingWriter;
        let mut err = Vec::new();
        let code = run(os(&["--help"]), &mut out, &mut err);
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::from(1)));
        let text = String::from_utf8(err).unwrap();
        assert!(text.contains("<stdout>"));
    }
}
