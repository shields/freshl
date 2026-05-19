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
    pub symlink_target: Option<PathBuf>,
    // True only when `kind == Symlink` and the target stats as a directory.
    // Read by the sort comparator so symlinks-to-dirs group with real dirs.
    pub symlink_target_is_dir: bool,
    // Filesystem identity of the *recorded* metadata (target under -L, link
    // otherwise). Read by `list_recursive`'s cycle check under -LR so a
    // symlink that resolves back into its own ancestor chain is skipped.
    pub dev: u64,
    pub ino: u64,
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
