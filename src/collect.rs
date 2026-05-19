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

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::entry::{Entry, EntryKind};

const S_IFMT: u32 = 0o170_000;
const S_IFSOCK: u32 = 0o140_000;
const S_IFLNK: u32 = 0o120_000;
const S_IFREG: u32 = 0o100_000;
const S_IFBLK: u32 = 0o060_000;
const S_IFDIR: u32 = 0o040_000;
const S_IFCHR: u32 = 0o020_000;
const S_IFIFO: u32 = 0o010_000;

/// Result of reading a directory: the successful child entries and any
/// per-child stat failures attributed to the failing child path.
#[derive(Debug, Default)]
pub struct DirListing {
    pub entries: Vec<Entry>,
    pub errors: Vec<(PathBuf, io::Error)>,
}

/// Read `path` as a directory and return one [`Entry`] per child that could
/// be stat'd, along with per-child errors for those that could not.
///
/// With `follow_symlinks`, each child's metadata is taken from its target
/// (via `stat`) instead of the link itself (`lstat`); see [`entry_for_path`].
///
/// # Errors
///
/// Returns the underlying I/O error if `path` itself cannot be opened as a
/// directory or iterated. Per-child stat failures are accumulated in
/// `DirListing::errors` rather than aborting the listing, so an unreadable
/// individual file doesn't hide the rest of the directory's contents.
pub fn collect_directory(path: &Path, follow_symlinks: bool) -> io::Result<DirListing> {
    let mut iter = fs::read_dir(path)?.map(|r| r.map(|de| de.path()));
    Ok(process_paths(&mut iter, path, follow_symlinks))
}

// Takes a `&mut dyn Iterator` so the function compiles to a single
// instantiation; generic monomorphization would otherwise leave one match arm
// dead in each instantiation, which trips per-instantiation line coverage even
// when both arms are exercised across tests. A trait-object reference avoids
// the heap allocation a `Box<dyn …>` would impose on every directory read.
fn process_paths(
    iter: &mut dyn Iterator<Item = io::Result<PathBuf>>,
    parent: &Path,
    follow_symlinks: bool,
) -> DirListing {
    let mut listing = DirListing::default();
    for r in iter {
        match r {
            Ok(child) => match entry_for_path(&child, follow_symlinks) {
                Ok(e) => listing.entries.push(e),
                Err(source) => listing.errors.push((child, source)),
            },
            Err(source) => listing.errors.push((parent.to_path_buf(), source)),
        }
    }
    listing
}

/// Build an [`Entry`] for a single path.
///
/// By default uses `lstat` semantics: a symlink is reported as a symlink with
/// the link's own metadata and the target name attached. With
/// `follow_symlinks`, a symlink whose target can be `stat(2)`'d is reported
/// as the *target*: target mode/owner/size and the target's kind. Broken
/// symlinks under `follow_symlinks` fall back to the lstat representation so
/// the row still appears in the listing (matching `find -L` semantics).
///
/// # Errors
///
/// Returns the underlying I/O error if `path` does not exist or its metadata
/// cannot be read.
pub fn entry_for_path(path: &Path, follow_symlinks: bool) -> io::Result<Entry> {
    let lmeta = fs::symlink_metadata(path)?;
    let lkind = classify(lmeta.mode());

    if lkind != EntryKind::Symlink {
        return Ok(make_entry(path, &lmeta, lkind, None, false));
    }

    // `fs::metadata` is `stat(2)`; symlink cycles surface as ELOOP and the
    // `Err` is treated as "target unreachable", so the kernel's MAXSYMLINKS
    // bounds the work.
    let target_meta = fs::metadata(path).ok();

    if follow_symlinks && let Some(tmeta) = &target_meta {
        let tkind = classify(tmeta.mode());
        return Ok(make_entry(path, tmeta, tkind, None, false));
    }

    // If `read_link` fails on a path lstat'd as a symlink (rare — usually a
    // TOCTOU race or unusual filesystem), keep the entry so the user still
    // sees the symlink name in the listing; we just leave `symlink_target`
    // empty rather than dropping the entry entirely.
    let symlink_target = fs::read_link(path).ok();
    let target_is_dir = target_meta.is_some_and(|m| m.is_dir());
    Ok(make_entry(
        path,
        &lmeta,
        lkind,
        symlink_target,
        target_is_dir,
    ))
}

fn make_entry(
    path: &Path,
    meta: &fs::Metadata,
    kind: EntryKind,
    symlink_target: Option<PathBuf>,
    symlink_target_is_dir: bool,
) -> Entry {
    let name = path.file_name().map_or_else(
        || path.as_os_str().to_os_string(),
        std::ffi::OsStr::to_os_string,
    );
    Entry {
        name,
        path: path.to_path_buf(),
        kind,
        mode: meta.mode(),
        nlink: meta.nlink(),
        uid: meta.uid(),
        gid: meta.gid(),
        size: meta.size(),
        rdev: meta.rdev(),
        mtime: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        symlink_target,
        symlink_target_is_dir,
        dev: meta.dev(),
        ino: meta.ino(),
    }
}

#[must_use]
pub const fn classify(mode: u32) -> EntryKind {
    match mode & S_IFMT {
        S_IFDIR => EntryKind::Directory,
        S_IFLNK => EntryKind::Symlink,
        S_IFCHR => EntryKind::CharDevice,
        S_IFBLK => EntryKind::BlockDevice,
        S_IFIFO => EntryKind::Fifo,
        S_IFSOCK => EntryKind::Socket,
        S_IFREG => EntryKind::RegularFile,
        _ => EntryKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        S_IFBLK, S_IFCHR, S_IFDIR, S_IFIFO, S_IFLNK, S_IFREG, S_IFSOCK, classify,
        collect_directory, entry_for_path, process_paths,
    };
    use crate::entry::EntryKind;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use tempfile::tempdir;

    #[test]
    fn classify_recognises_every_posix_type() {
        assert_eq!(classify(S_IFDIR | 0o755), EntryKind::Directory);
        assert_eq!(classify(S_IFREG | 0o644), EntryKind::RegularFile);
        assert_eq!(classify(S_IFLNK | 0o777), EntryKind::Symlink);
        assert_eq!(classify(S_IFCHR | 0o666), EntryKind::CharDevice);
        assert_eq!(classify(S_IFBLK | 0o660), EntryKind::BlockDevice);
        assert_eq!(classify(S_IFIFO | 0o644), EntryKind::Fifo);
        assert_eq!(classify(S_IFSOCK | 0o755), EntryKind::Socket);
        assert_eq!(classify(0), EntryKind::Other);
    }

    #[test]
    fn collect_lists_all_entries_including_hidden() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a"), b"hello").unwrap();
        fs::write(dir.path().join(".hidden"), b"hi").unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();

        let mut listing = collect_directory(dir.path(), false).unwrap();
        listing.entries.sort_by(|x, y| x.name.cmp(&y.name));
        let names: Vec<_> = listing
            .entries
            .iter()
            .map(|e| e.name.to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![".hidden", "a", "sub"]);
        assert!(listing.errors.is_empty());

        let sub = listing.entries.iter().find(|e| e.name == "sub").unwrap();
        assert_eq!(sub.kind, EntryKind::Directory);

        let a = listing.entries.iter().find(|e| e.name == "a").unwrap();
        assert_eq!(a.kind, EntryKind::RegularFile);
        assert_eq!(a.size, 5);
    }

    #[test]
    fn collect_records_per_child_stat_failure_without_aborting() {
        let dir = tempdir().unwrap();
        let inner = dir.path().join("inner");
        fs::create_dir(&inner).unwrap();
        fs::write(inner.join("a"), b"hi").unwrap();
        // r-- on the directory itself: `read_dir` can read names, but resolving
        // each child path for `lstat` requires the +x bit, which we've cleared.
        let mut p = fs::metadata(&inner).unwrap().permissions();
        p.set_mode(0o400);
        fs::set_permissions(&inner, p).unwrap();

        let listing = collect_directory(&inner, false);

        let mut p = fs::metadata(&inner).unwrap().permissions();
        p.set_mode(0o755);
        fs::set_permissions(&inner, p).unwrap();

        let listing = listing.unwrap();
        assert!(listing.entries.is_empty());
        assert_eq!(listing.errors.len(), 1);
        assert_eq!(listing.errors[0].0, inner.join("a"));
    }

    #[test]
    fn entry_for_path_does_not_follow_symlink() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("target");
        fs::write(&target, b"hi").unwrap();
        let link = dir.path().join("link");
        symlink(&target, &link).unwrap();

        let entry = entry_for_path(&link, false).unwrap();
        assert_eq!(entry.kind, EntryKind::Symlink);
        assert_eq!(entry.symlink_target.as_deref(), Some(target.as_path()));
        assert!(!entry.symlink_target_is_dir);
    }

    #[test]
    fn entry_for_path_symlink_to_directory_sets_target_is_dir() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("target_dir");
        fs::create_dir(&target).unwrap();
        let link = dir.path().join("link_to_dir");
        symlink(&target, &link).unwrap();

        let entry = entry_for_path(&link, false).unwrap();
        assert_eq!(entry.kind, EntryKind::Symlink);
        assert!(entry.symlink_target_is_dir);
    }

    #[test]
    fn entry_for_path_broken_symlink_still_classifies() {
        let dir = tempdir().unwrap();
        let link = dir.path().join("dangling");
        symlink(dir.path().join("nope"), &link).unwrap();
        let entry = entry_for_path(&link, false).unwrap();
        assert_eq!(entry.kind, EntryKind::Symlink);
        assert!(entry.symlink_target.is_some());
        assert!(!entry.symlink_target_is_dir);
    }

    #[test]
    fn entry_for_path_symlink_cycle_does_not_loop_and_is_not_dir() {
        // Pins the loop-safety contract: ELOOP from stat(2) maps to false,
        // so a hand-rolled symlink walk could not silently replace it.
        let dir = tempdir().unwrap();
        let a = dir.path().join("loop_a");
        let b = dir.path().join("loop_b");
        symlink(&b, &a).unwrap();
        symlink(&a, &b).unwrap();
        let entry = entry_for_path(&a, false).unwrap();
        assert_eq!(entry.kind, EntryKind::Symlink);
        assert!(!entry.symlink_target_is_dir);
    }

    #[test]
    fn entry_for_path_follow_reports_target_metadata_for_file() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("target");
        fs::write(&target, b"contents").unwrap();
        let link = dir.path().join("link");
        symlink(&target, &link).unwrap();

        let entry = entry_for_path(&link, true).unwrap();
        assert_eq!(entry.kind, EntryKind::RegularFile);
        assert!(entry.symlink_target.is_none());
        assert!(!entry.symlink_target_is_dir);
        assert_eq!(entry.size, b"contents".len() as u64);
    }

    #[test]
    fn entry_for_path_follow_reports_target_kind_for_directory() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("target_dir");
        fs::create_dir(&target).unwrap();
        let link = dir.path().join("link_to_dir");
        symlink(&target, &link).unwrap();

        let entry = entry_for_path(&link, true).unwrap();
        assert_eq!(entry.kind, EntryKind::Directory);
        assert!(entry.symlink_target.is_none());
    }

    #[test]
    fn entry_for_path_follow_falls_back_on_broken_symlink() {
        let dir = tempdir().unwrap();
        let link = dir.path().join("dangling");
        symlink(dir.path().join("nope"), &link).unwrap();
        let entry = entry_for_path(&link, true).unwrap();
        assert_eq!(entry.kind, EntryKind::Symlink);
        assert!(entry.symlink_target.is_some());
        assert!(!entry.symlink_target_is_dir);
    }

    #[test]
    fn entry_for_path_follow_is_noop_for_regular_file() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("plain");
        fs::write(&file, b"x").unwrap();
        let with = entry_for_path(&file, true).unwrap();
        let without = entry_for_path(&file, false).unwrap();
        assert_eq!(with.kind, without.kind);
        assert_eq!(with.size, without.size);
    }

    #[test]
    fn collect_errors_on_missing_path() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("nope");
        assert!(collect_directory(&missing, false).is_err());
    }

    #[test]
    fn entry_for_path_errors_on_missing() {
        let dir = tempdir().unwrap();
        assert!(entry_for_path(&dir.path().join("nope"), false).is_err());
    }

    #[test]
    fn root_path_has_a_name() {
        let entry = entry_for_path(std::path::Path::new("/"), false).unwrap();
        assert!(!entry.name.is_empty());
    }

    #[test]
    fn process_paths_records_iter_errors_against_parent() {
        use std::io;
        use std::path::Path;
        let synthetic: Vec<io::Result<std::path::PathBuf>> =
            vec![Err(io::Error::other("synthetic"))];
        let mut iter = synthetic.into_iter();
        let listing = process_paths(&mut iter, Path::new("/synthetic-parent"), false);
        assert!(listing.entries.is_empty());
        assert_eq!(listing.errors.len(), 1);
        assert_eq!(listing.errors[0].0, Path::new("/synthetic-parent"));
    }
}
