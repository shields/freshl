use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;

pub fn run<I>(_raw: I, _stdout: &mut dyn Write, _stderr: &mut dyn Write) -> ExitCode
where
    I: IntoIterator<Item = OsString>,
{
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::run;
    use std::ffi::OsString;

    fn code_repr(code: std::process::ExitCode) -> String {
        format!("{code:?}")
    }

    #[test]
    fn run_returns_success_when_invoked() {
        let args: Vec<OsString> = vec![];
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(args, &mut out, &mut err);
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
        assert!(out.is_empty());
        assert!(err.is_empty());
    }
}
