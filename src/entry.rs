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
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Directory,
    RegularFile,
    Symlink,
    CharDevice,
    BlockDevice,
    Fifo,
    Socket,
    Other,
}

impl EntryKind {
    #[must_use]
    pub const fn type_char(self) -> char {
        match self {
            Self::Directory => 'd',
            Self::RegularFile => ' ',
            Self::Symlink => 'l',
            Self::CharDevice => 'c',
            Self::BlockDevice => 'b',
            Self::Fifo => 'p',
            Self::Socket => 's',
            Self::Other => '?',
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: OsString,
    pub path: PathBuf,
    pub kind: EntryKind,
    pub mode: u32,
    pub nlink: u64,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub rdev: u64,
    pub mtime: SystemTime,
    // Populated only on the broken-symlink fallback path, where `stat` failed
    // and we kept the lstat representation so the row still appears. Healthy
    // symlinks express their target through `follow_chain` instead.
    pub symlink_target: Option<PathBuf>,
    // Filesystem identity of the *recorded* metadata (target for resolved
    // symlinks, link for broken ones). Read by `list_recursive`'s cycle check
    // so a symlink that resolves back into its own ancestor chain is skipped.
    pub dev: u64,
    pub ino: u64,
    // Readlink targets traversed while resolving this row. `[0]` is
    // `readlink(name)`; `last()` is the final non-symlink the chain
    // terminates in. Empty for non-symlinks and for broken-symlink fallbacks
    // (the chain isn't built when `stat` failed).
    pub follow_chain: Vec<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::EntryKind;

    #[test]
    fn type_char_covers_all_kinds() {
        assert_eq!(EntryKind::Directory.type_char(), 'd');
        assert_eq!(EntryKind::RegularFile.type_char(), ' ');
        assert_eq!(EntryKind::Symlink.type_char(), 'l');
        assert_eq!(EntryKind::CharDevice.type_char(), 'c');
        assert_eq!(EntryKind::BlockDevice.type_char(), 'b');
        assert_eq!(EntryKind::Fifo.type_char(), 'p');
        assert_eq!(EntryKind::Socket.type_char(), 's');
        assert_eq!(EntryKind::Other.type_char(), '?');
    }
}
