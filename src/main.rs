#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use std::process::ExitCode;

#[cfg_attr(coverage_nightly, coverage(off))]
fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let mut stdout = anstream::stdout();
    let mut stderr = anstream::stderr();
    freshl::run(args, &mut stdout, &mut stderr)
}
