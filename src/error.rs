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

use std::fmt;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Debug)]
pub enum Error {
    Usage(String),
    StdoutIo(std::io::Error),
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl Error {
    #[must_use]
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::Usage(_) => ExitCode::from(2),
            Self::StdoutIo(_) | Self::Io { .. } => ExitCode::from(1),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(msg) => write!(f, "freshl: {msg}"),
            Self::StdoutIo(source) => write!(f, "freshl: <stdout>: {source}"),
            Self::Io { path, source } => write!(f, "freshl: {}: {source}", path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Error;
    use std::path::PathBuf;
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
            path: PathBuf::from("thing"),
            source: std::io::Error::other("boom"),
        };
        assert_eq!(code_byte(err.exit_code()), code_byte(ExitCode::from(1)));
        let rendered = format!("{err}");
        assert!(rendered.contains("thing"));
        assert!(rendered.contains("boom"));
    }

    #[test]
    fn stdout_io_uses_exit_code_one_and_labels_stdout() {
        let err = Error::StdoutIo(std::io::Error::other("write fail"));
        assert_eq!(code_byte(err.exit_code()), code_byte(ExitCode::from(1)));
        let rendered = format!("{err}");
        assert!(rendered.contains("<stdout>"));
        assert!(rendered.contains("write fail"));
    }
}
