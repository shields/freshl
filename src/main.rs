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

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use std::process::ExitCode;

#[cfg_attr(coverage_nightly, coverage(off))]
fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let mut stdout = anstream::stdout();
    let mut stderr = anstream::stderr();
    freshl::run(args, &mut stdout, &mut stderr)
}
