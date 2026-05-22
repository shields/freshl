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

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use gix::bstr::{BStr, BString};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PorcelainCode {
    pub index: char,
    pub worktree: char,
}

impl PorcelainCode {
    pub const BLANK: Self = Self {
        index: ' ',
        worktree: ' ',
    };
    pub const CLEAN: Self = Self {
        index: '○',
        worktree: ' ',
    };
    pub const UNTRACKED: Self = Self {
        index: '?',
        worktree: '?',
    };
    pub const IGNORED: Self = Self {
        index: '·',
        worktree: '·',
    };
    pub const MODIFIED_WORKTREE: Self = Self {
        index: ' ',
        worktree: '●',
    };
    pub const DELETED_WORKTREE: Self = Self {
        index: ' ',
        worktree: '▽',
    };
    pub const TYPE_CHANGE_WORKTREE: Self = Self {
        index: ' ',
        worktree: '≈',
    };
    pub const RENAMED: Self = Self {
        index: '→',
        worktree: ' ',
    };
    pub const COPIED: Self = Self {
        index: '⇉',
        worktree: ' ',
    };
    pub const RENAMED_WORKTREE: Self = Self {
        index: ' ',
        worktree: '→',
    };
    pub const COPIED_WORKTREE: Self = Self {
        index: ' ',
        worktree: '⇉',
    };
    pub const UNMERGED: Self = Self {
        index: '✘',
        worktree: '✘',
    };
    pub const DIRTY_SUBTREE: Self = Self {
        index: '⋯',
        worktree: ' ',
    };

    #[must_use]
    pub const fn with_index(self, idx: char) -> Self {
        Self {
            index: idx,
            worktree: self.worktree,
        }
    }

    /// The single glyph rendered for this code: worktree wins, index is the fallback.
    #[must_use]
    pub const fn glyph(self) -> char {
        if self.worktree == ' ' {
            self.index
        } else {
            self.worktree
        }
    }
}

#[derive(Debug, Default)]
pub struct Snapshot {
    pub root: PathBuf,
    pub statuses: HashMap<PathBuf, PorcelainCode>,
    dirty_ancestors: HashSet<PathBuf>,
}

impl Snapshot {
    /// Resolve `path` to its `PorcelainCode`, defaulting to [`PorcelainCode::CLEAN`].
    ///
    /// Path normalisation tries lexical absolutisation first (cheap, preserves
    /// symlink identity for entries that are themselves symlinks). If the
    /// lexical result fails to land inside `self.root` or contains unresolved
    /// `..` components, it falls back to [`std::fs::canonicalize`] — that
    /// covers symlinked workdirs and `freshl ..` style paths.
    #[must_use]
    pub fn lookup(&self, path: &Path) -> PorcelainCode {
        self.relativize(path)
            .map_or(PorcelainCode::CLEAN, |rel| self.lookup_rel(&rel))
    }

    fn lookup_rel(&self, rel: &Path) -> PorcelainCode {
        if let Some(code) = self.statuses.get(rel).copied() {
            return code;
        }
        for ancestor in iter_ancestors(rel) {
            if let Some(code) = self.statuses.get(ancestor).copied()
                && (code == PorcelainCode::UNTRACKED || code == PorcelainCode::IGNORED)
            {
                return code;
            }
        }
        PorcelainCode::CLEAN
    }

    #[must_use]
    pub fn is_ignored(&self, path: &Path) -> bool {
        self.lookup(path) == PorcelainCode::IGNORED
    }

    /// Ignored descendants don't count — otherwise vendored/build trees would
    /// flag every ancestor.
    #[must_use]
    pub fn has_dirty_descendants(&self, path: &Path) -> bool {
        let Some(rel) = self.relativize(path) else {
            return false;
        };
        self.dirty_ancestors.contains(&rel)
    }

    /// Returns `DIRTY_SUBTREE` for a tracked-clean directory whose subtree
    /// has dirty descendants; otherwise behaves like [`Self::lookup`].
    /// Single path-normalisation for both checks.
    #[must_use]
    pub fn display_code_for(&self, path: &Path, is_directory: bool) -> PorcelainCode {
        let Some(rel) = self.relativize(path) else {
            return PorcelainCode::CLEAN;
        };
        let direct = self.lookup_rel(&rel);
        if direct == PorcelainCode::CLEAN && is_directory && self.dirty_ancestors.contains(&rel) {
            PorcelainCode::DIRTY_SUBTREE
        } else {
            direct
        }
    }

    fn relativize(&self, path: &Path) -> Option<PathBuf> {
        let abs = std::path::absolute(path).ok();
        let candidate: &Path = abs.as_deref().unwrap_or(path);
        if let Ok(rel) = candidate.strip_prefix(&self.root) {
            // `std::path::absolute` strips all `.` components (both leading
            // and interior) on POSIX, so we only have to watch for `..`,
            // which it preserves and which would mis-key the status lookup.
            let has_dotdot = rel.components().any(|c| matches!(c, Component::ParentDir));
            if !has_dotdot {
                return Some(rel.to_path_buf());
            }
        }
        // Canonicalise the parent and re-attach the leaf so directory
        // symlinks (e.g. macOS `/var` → `/private/var`) and `..` components
        // resolve, but a symlinked entry isn't dereferenced into its target.
        // When the path has no separable parent+name (single-component, `.`,
        // `..`, `/`), canonicalising the whole path is safe — those forms
        // can't themselves be the symlinked entry we're trying to look up.
        let resolved = match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => {
                let to_canon: &Path = if parent.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    parent
                };
                std::fs::canonicalize(to_canon).ok()?.join(name)
            }
            _ => std::fs::canonicalize(path).ok()?,
        };
        resolved
            .strip_prefix(&self.root)
            .ok()
            .map(Path::to_path_buf)
    }
}

/// Caches snapshots keyed by canonical scope directory.
///
/// Each cache entry corresponds to one pathspec-limited status walk, so a
/// multi-target invocation only walks what it has to. Negative results (no
/// repository at the scope, or path normalisation failed) are cached as
/// `None`, so `freshl *` in a non-git directory doesn't re-traverse the
/// filesystem per target.
#[derive(Debug, Default)]
pub struct SnapshotCache {
    by_scope: HashMap<PathBuf, Option<Snapshot>>,
}

impl SnapshotCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn for_target(&mut self, target: &Path) -> Option<&Snapshot> {
        let scope = normalize_existing(&scope_dir(target))?;
        if !self.by_scope.contains_key(&scope) {
            let snapshot = build_snapshot(&scope);
            self.by_scope.insert(scope.clone(), snapshot);
        }
        self.by_scope.get(&scope)?.as_ref()
    }
}

fn build_snapshot(scope: &Path) -> Option<Snapshot> {
    let repo = gix::discover(scope).ok()?;
    let workdir = normalize_existing(repo.workdir()?)?;
    let pathspec = pathspec_for(scope, &workdir)?;
    let statuses = collect_statuses(&repo, pathspec).unwrap_or_default();
    let dirty_ancestors = compute_dirty_ancestors(&statuses);
    Some(Snapshot {
        root: workdir,
        statuses,
        dirty_ancestors,
    })
}

fn scope_dir(target: &Path) -> PathBuf {
    if target.is_dir() {
        return target.to_path_buf();
    }
    let parent = target.parent().filter(|p| !p.as_os_str().is_empty());
    parent.map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// Canonicalise where possible (resolves symlinks and `..`); fall back to
/// lexical absolutisation. Returns `None` only when both `canonicalize` and
/// `std::path::absolute` fail — on Unix that essentially means the CWD has
/// been deleted out from under us, at which point we give up rather than
/// guess.
fn normalize_existing(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path)
        .ok()
        .or_else(|| std::path::absolute(path).ok())
}

fn pathspec_for(scope: &Path, workdir: &Path) -> Option<Vec<BString>> {
    let rel = scope.strip_prefix(workdir).ok()?;
    let bytes = rel.as_os_str().as_bytes();
    if bytes.is_empty() {
        Some(Vec::new())
    } else {
        Some(vec![BString::from(bytes.to_vec())])
    }
}

fn rela_to_pathbuf(b: &BStr) -> PathBuf {
    // gix paths are raw bytes; on Unix go through OsStr directly so non-UTF-8
    // names survive (to_os_str_lossy would replace invalid sequences with U+FFFD).
    PathBuf::from(OsStr::from_bytes(b.as_ref()))
}

#[must_use]
pub fn discover(start: &Path) -> Option<Snapshot> {
    let repo = gix::discover(start).ok()?;
    let workdir = normalize_existing(repo.workdir()?)?;
    let statuses = collect_statuses(&repo, Vec::new()).unwrap_or_default();
    let dirty_ancestors = compute_dirty_ancestors(&statuses);
    Some(Snapshot {
        root: workdir,
        statuses,
        dirty_ancestors,
    })
}

fn compute_dirty_ancestors(statuses: &HashMap<PathBuf, PorcelainCode>) -> HashSet<PathBuf> {
    let mut out = HashSet::new();
    for (path, code) in statuses {
        if *code == PorcelainCode::CLEAN || *code == PorcelainCode::IGNORED {
            continue;
        }
        for ancestor in iter_ancestors(path) {
            out.insert(ancestor.to_path_buf());
        }
    }
    out
}

// Yields strict ancestors of `rel`, including the empty path (which represents
// the repository root after `relativize`). The repository root itself must
// appear in `dirty_ancestors` so `freshl -d <root>` flags a dirty tree; the
// extra `statuses.get("")` in `lookup_rel` is a harmless miss.
fn iter_ancestors(rel: &Path) -> impl Iterator<Item = &Path> {
    rel.ancestors().skip(1)
}

fn collect_statuses(
    repo: &gix::Repository,
    pathspec: Vec<BString>,
) -> Result<HashMap<PathBuf, PorcelainCode>, Box<dyn std::error::Error>> {
    let mut out: HashMap<PathBuf, PorcelainCode> = HashMap::new();

    let platform = repo
        .status(gix::progress::Discard)?
        // Collapsed: an entirely-untracked or -ignored directory is emitted as a
        // single entry; files within are absent from the map and inherit via
        // `Snapshot::lookup`'s ancestor walk.
        .untracked_files(gix::status::UntrackedFiles::Collapsed)
        .index_worktree_rewrites(gix::diff::Rewrites::default())
        .dirwalk_options(|opts| opts.emit_ignored(Some(gix::dir::walk::EmissionMode::Matching)));

    let iter = platform.into_iter(pathspec)?;
    for item in iter {
        let item = item?;
        match item {
            gix::status::Item::IndexWorktree(iw) => {
                handle_index_worktree(&iw, &mut out);
            }
            gix::status::Item::TreeIndex(change) => {
                handle_tree_index(&change, &mut out);
            }
        }
    }

    Ok(out)
}

fn handle_index_worktree(
    item: &gix::status::index_worktree::Item,
    out: &mut HashMap<PathBuf, PorcelainCode>,
) {
    use gix::status::plumbing::index_as_worktree::{Change as IwChange, EntryStatus};
    match item {
        gix::status::index_worktree::Item::Modification {
            rela_path, status, ..
        } => {
            let path = rela_to_pathbuf(rela_path.as_ref());
            let code = match status {
                EntryStatus::Change(IwChange::Removed) => PorcelainCode::DELETED_WORKTREE,
                EntryStatus::Change(IwChange::Type { .. }) => PorcelainCode::TYPE_CHANGE_WORKTREE,
                EntryStatus::Conflict { .. } => PorcelainCode::UNMERGED,
                _ => PorcelainCode::MODIFIED_WORKTREE,
            };
            let prev = out.get(&path).copied();
            out.insert(path, merge(prev, code));
        }
        gix::status::index_worktree::Item::DirectoryContents { entry, .. } => {
            let path = rela_to_pathbuf(entry.rela_path.as_ref());
            let code = match entry.status {
                gix::dir::entry::Status::Ignored(_) => PorcelainCode::IGNORED,
                _ => PorcelainCode::UNTRACKED,
            };
            out.insert(path, code);
        }
        gix::status::index_worktree::Item::Rewrite {
            dirwalk_entry,
            copy,
            ..
        } => {
            let path = rela_to_pathbuf(dirwalk_entry.rela_path.as_ref());
            let code = rewrite_code(*copy);
            let prev = out.get(&path).copied();
            out.insert(path, merge(prev, code));
        }
    }
}

fn handle_tree_index(change: &gix::diff::index::Change, out: &mut HashMap<PathBuf, PorcelainCode>) {
    let (rel, idx_char) = match change {
        gix::diff::index::Change::Addition { location, .. } => (location, '+'),
        gix::diff::index::Change::Deletion { location, .. } => (location, '▽'),
        gix::diff::index::Change::Modification { location, .. } => (location, '●'),
        gix::diff::index::Change::Rewrite { location, .. } => (location, '→'),
    };
    let path = rela_to_pathbuf(rel);
    let existing = out.get(&path).copied().unwrap_or(PorcelainCode::BLANK);
    out.insert(path, existing.with_index(idx_char));
}

const fn rewrite_code(copy: bool) -> PorcelainCode {
    // Worktree-only rewrites: mark the worktree column so they don't look
    // like staged renames/copies (those go through `handle_tree_index`).
    if copy {
        PorcelainCode::COPIED_WORKTREE
    } else {
        PorcelainCode::RENAMED_WORKTREE
    }
}

fn merge(prev: Option<PorcelainCode>, next: PorcelainCode) -> PorcelainCode {
    prev.map_or(next, |p| PorcelainCode {
        index: if p.index == ' ' { next.index } else { p.index },
        worktree: if next.worktree == ' ' {
            p.worktree
        } else {
            next.worktree
        },
    })
}

#[cfg(test)]
mod tests {
    use super::{PorcelainCode, Snapshot, SnapshotCache, discover, merge};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::{Mutex, MutexGuard};
    use tempfile::TempDir;

    // The lock serialises `git` subprocesses so cargo's parallel test runner
    // can't have two of them sharing index state via environment.
    static GIT_LOCK: Mutex<()> = Mutex::new(());

    fn run_git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("HOME", dir)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    fn fresh_repo() -> (MutexGuard<'static, ()>, TempDir) {
        let guard = GIT_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init", "-q", "-b", "main"]);
        run_git(
            dir.path(),
            &["config", "user.email", "test@example.invalid"],
        );
        run_git(dir.path(), &["config", "user.name", "Test"]);
        (guard, dir)
    }

    fn snapshot_for(dir: &Path) -> Snapshot {
        discover(dir).expect("repo present")
    }

    fn status_at(snap: &Snapshot, rel: &str) -> PorcelainCode {
        snap.lookup(&snap.root.join(rel))
    }

    #[test]
    fn rewrite_code_distinguishes_copy_and_rename() {
        use super::rewrite_code;
        assert_eq!(rewrite_code(true), PorcelainCode::COPIED_WORKTREE);
        assert_eq!(rewrite_code(false), PorcelainCode::RENAMED_WORKTREE);
    }

    #[test]
    fn merge_returns_next_when_no_prior() {
        let m = merge(None, PorcelainCode::UNTRACKED);
        assert_eq!(m, PorcelainCode::UNTRACKED);
    }

    #[test]
    fn merge_keeps_prior_index_and_takes_next_worktree() {
        let prev = PorcelainCode {
            index: '+',
            worktree: ' ',
        };
        let m = merge(Some(prev), PorcelainCode::MODIFIED_WORKTREE);
        assert_eq!(m.index, '+');
        assert_eq!(m.worktree, '●');
    }

    #[test]
    fn merge_keeps_prior_worktree_when_next_is_blank() {
        let prev = PorcelainCode {
            index: ' ',
            worktree: '▽',
        };
        let next = PorcelainCode {
            index: '●',
            worktree: ' ',
        };
        let m = merge(Some(prev), next);
        assert_eq!(m.index, '●');
        assert_eq!(m.worktree, '▽');
    }

    #[test]
    fn lookup_returns_clean_for_unknown_path() {
        let snap = Snapshot {
            root: PathBuf::from("/repo"),
            statuses: HashMap::new(),
            ..Default::default()
        };
        assert_eq!(snap.lookup(Path::new("/repo/file")), PorcelainCode::CLEAN);
        assert_eq!(snap.lookup(Path::new("/elsewhere")), PorcelainCode::CLEAN);
    }

    #[test]
    fn lookup_returns_stored_code_for_known_relative_path() {
        let mut statuses = HashMap::new();
        statuses.insert(PathBuf::from("a"), PorcelainCode::UNTRACKED);
        let snap = Snapshot {
            root: PathBuf::from("/repo"),
            statuses,
            ..Default::default()
        };
        assert_eq!(snap.lookup(Path::new("/repo/a")), PorcelainCode::UNTRACKED);
    }

    #[test]
    fn is_ignored_only_true_for_ignored_code() {
        let mut statuses = HashMap::new();
        statuses.insert(PathBuf::from("ig"), PorcelainCode::IGNORED);
        statuses.insert(PathBuf::from("un"), PorcelainCode::UNTRACKED);
        let snap = Snapshot {
            root: PathBuf::from("/repo"),
            statuses,
            ..Default::default()
        };
        assert!(snap.is_ignored(Path::new("/repo/ig")));
        assert!(!snap.is_ignored(Path::new("/repo/un")));
        assert!(!snap.is_ignored(Path::new("/repo/missing")));
    }

    #[test]
    fn discover_returns_none_outside_repo() {
        let dir = tempfile::tempdir().unwrap();
        assert!(discover(dir.path()).is_none());
    }

    #[test]
    fn discover_returns_some_inside_repo() {
        let dir = tempfile::tempdir().unwrap();
        let _repo = gix::init(dir.path()).unwrap();
        let snap = discover(dir.path()).unwrap();
        let expected = std::fs::canonicalize(dir.path()).unwrap();
        let actual = std::fs::canonicalize(&snap.root).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn discover_reports_untracked_file() {
        let (_g, dir) = fresh_repo();
        std::fs::write(dir.path().join("new"), b"x").unwrap();
        let snap = snapshot_for(dir.path());
        assert_eq!(status_at(&snap, "new"), PorcelainCode::UNTRACKED);
    }

    #[test]
    fn discover_reports_ignored_file() {
        let (_g, dir) = fresh_repo();
        std::fs::write(dir.path().join(".gitignore"), b"hidden\n").unwrap();
        std::fs::write(dir.path().join("hidden"), b"x").unwrap();
        let snap = snapshot_for(dir.path());
        assert_eq!(status_at(&snap, "hidden"), PorcelainCode::IGNORED);
    }

    #[test]
    fn discover_reports_modified_worktree() {
        let (_g, dir) = fresh_repo();
        std::fs::write(dir.path().join("a"), b"hello\n").unwrap();
        run_git(dir.path(), &["add", "a"]);
        run_git(dir.path(), &["commit", "-q", "-m", "x"]);
        std::fs::write(dir.path().join("a"), b"different\n").unwrap();
        let snap = snapshot_for(dir.path());
        assert_eq!(status_at(&snap, "a"), PorcelainCode::MODIFIED_WORKTREE);
    }

    #[test]
    fn discover_reports_deleted_worktree() {
        let (_g, dir) = fresh_repo();
        std::fs::write(dir.path().join("b"), b"x").unwrap();
        run_git(dir.path(), &["add", "b"]);
        run_git(dir.path(), &["commit", "-q", "-m", "x"]);
        std::fs::remove_file(dir.path().join("b")).unwrap();
        let snap = snapshot_for(dir.path());
        assert_eq!(status_at(&snap, "b"), PorcelainCode::DELETED_WORKTREE);
    }

    #[test]
    fn discover_reports_staged_addition() {
        let (_g, dir) = fresh_repo();
        // The tree-vs-index diff needs at least one commit on HEAD to compare against.
        std::fs::write(dir.path().join("seed"), b"x").unwrap();
        run_git(dir.path(), &["add", "seed"]);
        run_git(dir.path(), &["commit", "-q", "-m", "seed"]);
        std::fs::write(dir.path().join("staged"), b"hi").unwrap();
        run_git(dir.path(), &["add", "staged"]);
        let snap = snapshot_for(dir.path());
        assert_eq!(status_at(&snap, "staged").index, '+');
    }

    #[test]
    fn discover_reports_staged_modification() {
        let (_g, dir) = fresh_repo();
        std::fs::write(dir.path().join("m"), b"one\n").unwrap();
        run_git(dir.path(), &["add", "m"]);
        run_git(dir.path(), &["commit", "-q", "-m", "m"]);
        std::fs::write(dir.path().join("m"), b"two\n").unwrap();
        run_git(dir.path(), &["add", "m"]);
        let snap = snapshot_for(dir.path());
        assert_eq!(status_at(&snap, "m").index, '●');
    }

    #[test]
    fn discover_reports_staged_deletion() {
        let (_g, dir) = fresh_repo();
        std::fs::write(dir.path().join("d"), b"x").unwrap();
        run_git(dir.path(), &["add", "d"]);
        run_git(dir.path(), &["commit", "-q", "-m", "d"]);
        run_git(dir.path(), &["rm", "-q", "d"]);
        let snap = snapshot_for(dir.path());
        assert_eq!(status_at(&snap, "d").index, '▽');
    }

    #[test]
    fn discover_reports_rename_in_worktree() {
        let (_g, dir) = fresh_repo();
        // 40 lines so the similarity threshold inside gix's rewrite detector
        // is comfortably above the default 50% — otherwise the rename is
        // reported as an add+delete pair.
        let body = "line\n".repeat(40);
        std::fs::write(dir.path().join("from"), &body).unwrap();
        run_git(dir.path(), &["add", "from"]);
        run_git(dir.path(), &["commit", "-q", "-m", "from"]);
        std::fs::rename(dir.path().join("from"), dir.path().join("to")).unwrap();
        let snap = snapshot_for(dir.path());
        assert_eq!(status_at(&snap, "to"), PorcelainCode::RENAMED_WORKTREE);
    }

    #[test]
    fn discover_reports_staged_rename() {
        let (_g, dir) = fresh_repo();
        let body = "line\n".repeat(40);
        std::fs::write(dir.path().join("from"), &body).unwrap();
        run_git(dir.path(), &["add", "from"]);
        run_git(dir.path(), &["commit", "-q", "-m", "from"]);
        run_git(dir.path(), &["mv", "from", "to"]);
        let snap = snapshot_for(dir.path());
        assert_eq!(status_at(&snap, "to").index, '→');
    }

    #[test]
    fn discover_reports_unmerged_conflict() {
        let (_g, dir) = fresh_repo();
        std::fs::write(dir.path().join("c"), b"base\n").unwrap();
        run_git(dir.path(), &["add", "c"]);
        run_git(dir.path(), &["commit", "-q", "-m", "base"]);
        run_git(dir.path(), &["checkout", "-q", "-b", "other"]);
        std::fs::write(dir.path().join("c"), b"other\n").unwrap();
        run_git(dir.path(), &["commit", "-q", "-am", "other"]);
        run_git(dir.path(), &["checkout", "-q", "main"]);
        std::fs::write(dir.path().join("c"), b"main\n").unwrap();
        run_git(dir.path(), &["commit", "-q", "-am", "main"]);
        // git merge exits non-zero on conflict; ignore its status.
        let _ = Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["merge", "--no-edit", "-q", "other"])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("HOME", dir.path())
            .status()
            .unwrap();
        let snap = snapshot_for(dir.path());
        assert_eq!(status_at(&snap, "c"), PorcelainCode::UNMERGED);
    }

    #[test]
    fn discover_reports_type_change() {
        let (_g, dir) = fresh_repo();
        std::fs::write(dir.path().join("t"), b"file").unwrap();
        run_git(dir.path(), &["add", "t"]);
        run_git(dir.path(), &["commit", "-q", "-m", "t"]);
        std::fs::remove_file(dir.path().join("t")).unwrap();
        std::os::unix::fs::symlink("anything", dir.path().join("t")).unwrap();
        let snap = snapshot_for(dir.path());
        assert_eq!(status_at(&snap, "t"), PorcelainCode::TYPE_CHANGE_WORKTREE);
    }

    #[test]
    fn porcelain_codes_are_distinct() {
        let codes = [
            PorcelainCode::CLEAN,
            PorcelainCode::UNTRACKED,
            PorcelainCode::IGNORED,
            PorcelainCode::MODIFIED_WORKTREE,
            PorcelainCode::DELETED_WORKTREE,
            PorcelainCode::TYPE_CHANGE_WORKTREE,
            PorcelainCode::RENAMED,
            PorcelainCode::COPIED,
            PorcelainCode::RENAMED_WORKTREE,
            PorcelainCode::COPIED_WORKTREE,
            PorcelainCode::UNMERGED,
            PorcelainCode::DIRTY_SUBTREE,
            PorcelainCode::BLANK,
        ];
        for (i, a) in codes.iter().enumerate() {
            for b in &codes[i + 1..] {
                assert_ne!(a, b);
            }
        }
        let with = PorcelainCode::MODIFIED_WORKTREE.with_index('+');
        assert_eq!(with.index, '+');
        assert_eq!(with.worktree, '●');
    }

    #[test]
    fn lookup_inherits_untracked_from_collapsed_directory() {
        let mut statuses = HashMap::new();
        statuses.insert(PathBuf::from("dir"), PorcelainCode::UNTRACKED);
        let snap = Snapshot {
            root: PathBuf::from("/repo"),
            statuses,
            ..Default::default()
        };
        assert_eq!(
            snap.lookup(Path::new("/repo/dir/file")),
            PorcelainCode::UNTRACKED,
        );
        assert_eq!(
            snap.lookup(Path::new("/repo/dir/deeper/file")),
            PorcelainCode::UNTRACKED,
        );
        // A sibling outside the collapsed directory stays clean.
        assert_eq!(snap.lookup(Path::new("/repo/other")), PorcelainCode::CLEAN,);
    }

    #[test]
    fn lookup_inherits_ignored_but_not_other_statuses_from_ancestors() {
        let mut statuses = HashMap::new();
        statuses.insert(PathBuf::from("ig"), PorcelainCode::IGNORED);
        statuses.insert(PathBuf::from("mod"), PorcelainCode::MODIFIED_WORKTREE);
        let snap = Snapshot {
            root: PathBuf::from("/repo"),
            statuses,
            ..Default::default()
        };
        assert_eq!(
            snap.lookup(Path::new("/repo/ig/inside")),
            PorcelainCode::IGNORED,
        );
        // A child of a Modified file (nonsense for regular files but possible
        // for type-change cases) shouldn't inherit Modified.
        assert_eq!(
            snap.lookup(Path::new("/repo/mod/inside")),
            PorcelainCode::CLEAN,
        );
    }

    #[test]
    fn lookup_handles_relative_target_against_absolute_root() {
        let mut statuses = HashMap::new();
        statuses.insert(PathBuf::from("a.txt"), PorcelainCode::UNTRACKED);
        let snap = Snapshot {
            root: std::env::current_dir().unwrap(),
            statuses,
            ..Default::default()
        };
        // A relative path that absolutises against the current dir into the
        // workdir should still match its status.
        assert_eq!(snap.lookup(Path::new("a.txt")), PorcelainCode::UNTRACKED);
        assert_eq!(snap.lookup(Path::new("./a.txt")), PorcelainCode::UNTRACKED);
    }

    #[test]
    fn lookup_resolves_dotdot_via_canonicalize_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let canonical = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(canonical.join("file"), b"x").unwrap();
        std::fs::create_dir(canonical.join("sub")).unwrap();

        let mut statuses = HashMap::new();
        statuses.insert(PathBuf::from("file"), PorcelainCode::UNTRACKED);
        let snap = Snapshot {
            root: canonical.clone(),
            statuses,
            ..Default::default()
        };
        // The lexical strip succeeds but yields `sub/../file`; the canonicalize
        // fallback simplifies that to `file` so the lookup matches.
        let weird = canonical.join("sub").join("..").join("file");
        assert_eq!(snap.lookup(&weird), PorcelainCode::UNTRACKED);
    }

    #[test]
    fn lookup_returns_clean_when_canonicalize_lands_outside_root() {
        let dir = tempfile::tempdir().unwrap();
        let canonical = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(canonical.join("file"), b"x").unwrap();

        let snap = Snapshot {
            root: canonical.join("nonexistent_root"),
            statuses: HashMap::new(),
            ..Default::default()
        };
        // canonicalize succeeds but lands outside `root`, so the second
        // strip_prefix returns Err and `relativize` yields None.
        assert_eq!(snap.lookup(&canonical.join("file")), PorcelainCode::CLEAN,);
    }

    #[test]
    fn lookup_returns_clean_when_canonicalize_fails() {
        // A path containing `..` that doesn't resolve to a real filesystem
        // entry forces the canonicalize fallback to fail.
        let snap = Snapshot {
            root: PathBuf::from("/repo"),
            statuses: HashMap::new(),
            ..Default::default()
        };
        assert_eq!(
            snap.lookup(Path::new("/repo/missing/../also-missing")),
            PorcelainCode::CLEAN,
        );
    }

    #[test]
    fn lookup_preserves_leaf_symlink_via_parent_canonicalisation() {
        // The canonicalize fallback must NOT dereference a leaf symlink — if
        // it did, a symlinked entry would look up under its target's path
        // instead of its own. Build a workdir reached through a directory
        // symlink and confirm `lookup` resolves the leaf via the symlink's
        // own name.
        let dir = tempfile::tempdir().unwrap();
        let canonical = std::fs::canonicalize(dir.path()).unwrap();
        std::os::unix::fs::symlink("/dev/null", canonical.join("entry")).unwrap();
        // Put the directory symlink in a separate tempdir so its path can't
        // share a prefix with `canonical`. Otherwise on platforms where the
        // tempdir is already its own canonical path (Linux), the lexical
        // strip_prefix would succeed and yield `via_link/entry`, bypassing
        // the parent-canonicalisation fallback this test exists to cover.
        let link_parent = tempfile::tempdir().unwrap();
        let link_dir = link_parent.path().join("via_link");
        std::os::unix::fs::symlink(&canonical, &link_dir).unwrap();

        let mut statuses = HashMap::new();
        statuses.insert(PathBuf::from("entry"), PorcelainCode::TYPE_CHANGE_WORKTREE);
        let snap = Snapshot {
            root: canonical,
            statuses,
            ..Default::default()
        };
        // Path is reached via the directory symlink; lexical strip_prefix
        // fails because link_dir's path differs from the canonical root.
        // The parent-canonicalisation fallback must resolve `via_link` to
        // the real workdir AND preserve the `entry` leaf without following
        // its symlink to `/dev/null`.
        assert_eq!(
            snap.lookup(&link_dir.join("entry")),
            PorcelainCode::TYPE_CHANGE_WORKTREE,
        );
    }

    #[test]
    fn lookup_handles_single_component_relative_path() {
        // `Path::new("file").parent()` is `Some("")`; the fallback must
        // substitute "." so canonicalize doesn't choke on the empty path.
        // Use a root that doesn't include cwd so the lexical branch fails
        // and the parent-canonicalisation fallback fires; the test then
        // just confirms `relativize` does not panic and returns CLEAN.
        let snap = Snapshot {
            root: PathBuf::from("/definitely/not/the/cwd"),
            statuses: HashMap::new(),
            ..Default::default()
        };
        assert_eq!(snap.lookup(Path::new("solo")), PorcelainCode::CLEAN);
    }

    #[test]
    fn lookup_handles_path_with_no_file_name() {
        // `Path::new("/").file_name()` is `None`. The fallback must
        // canonicalise the whole path rather than panic via `?`.
        let dir = tempfile::tempdir().unwrap();
        let canonical = std::fs::canonicalize(dir.path()).unwrap();
        let snap = Snapshot {
            root: canonical,
            statuses: HashMap::new(),
            ..Default::default()
        };
        // A bare `..` has no file_name; lookup must not panic and must
        // return CLEAN (the parent dir of the tempdir is outside the root).
        assert_eq!(snap.lookup(Path::new("..")), PorcelainCode::CLEAN,);
    }

    #[test]
    fn lookup_returns_clean_when_outside_root() {
        let snap = Snapshot {
            root: PathBuf::from("/repo"),
            statuses: HashMap::new(),
            ..Default::default()
        };
        assert_eq!(
            snap.lookup(Path::new("/elsewhere/file")),
            PorcelainCode::CLEAN,
        );
    }

    #[test]
    fn snapshot_cache_reuses_entry_for_same_target() {
        let (_g, dir) = fresh_repo();
        let mut cache = SnapshotCache::new();
        let first_ptr: *const Snapshot = cache.for_target(dir.path()).unwrap();
        let second_ptr: *const Snapshot = cache.for_target(dir.path()).unwrap();
        assert!(std::ptr::eq(first_ptr, second_ptr));
    }

    #[test]
    fn snapshot_cache_walks_each_scope_independently() {
        let (_g, dir) = fresh_repo();
        // Make `a/` have tracked-clean content plus one untracked file, and
        // `b/` similar but distinguishable — that way the pathspec walks
        // actually have to descend (collapsed mode would otherwise emit the
        // whole subdir as a single untracked entry).
        let sub_a = dir.path().join("a");
        let sub_b = dir.path().join("b");
        std::fs::create_dir(&sub_a).unwrap();
        std::fs::create_dir(&sub_b).unwrap();
        std::fs::write(sub_a.join("tracked"), b"x").unwrap();
        std::fs::write(sub_b.join("tracked"), b"x").unwrap();
        run_git(dir.path(), &["add", "."]);
        run_git(dir.path(), &["commit", "-q", "-m", "init"]);
        std::fs::write(sub_a.join("only_a"), b"x").unwrap();
        std::fs::write(sub_b.join("only_b"), b"x").unwrap();

        let mut cache = SnapshotCache::new();
        let in_a: Vec<_> = cache
            .for_target(&sub_a)
            .unwrap()
            .statuses
            .keys()
            .cloned()
            .collect();
        let in_b: Vec<_> = cache
            .for_target(&sub_b)
            .unwrap()
            .statuses
            .keys()
            .cloned()
            .collect();
        assert!(in_a.iter().any(|p| p.ends_with("only_a")));
        assert!(!in_a.iter().any(|p| p.ends_with("only_b")));
        assert!(in_b.iter().any(|p| p.ends_with("only_b")));
        assert!(!in_b.iter().any(|p| p.ends_with("only_a")));
    }

    #[test]
    fn snapshot_cache_returns_none_outside_repo() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = SnapshotCache::new();
        assert!(cache.for_target(dir.path()).is_none());
    }

    #[test]
    fn snapshot_cache_caches_negative_results() {
        // A non-git directory should only cause one `gix::discover` traversal
        // regardless of how many targets resolve into it. We exercise the
        // public API and then sneak in via the same cache key to confirm the
        // sentinel `None` is recorded.
        let dir = tempfile::tempdir().unwrap();
        let canon = std::fs::canonicalize(dir.path()).unwrap();
        let mut cache = SnapshotCache::new();
        assert!(cache.for_target(dir.path()).is_none());
        // Second call lands on the same canonical scope; the negative result
        // is returned from the map rather than from a fresh walk.
        assert!(cache.for_target(dir.path()).is_none());
        assert!(cache.by_scope.contains_key(&canon));
        assert!(cache.by_scope.get(&canon).unwrap().is_none());
    }

    #[test]
    fn scope_dir_for_bare_filename_returns_current_dir() {
        use super::scope_dir;
        // `target.parent()` for a bare filename is `Some("")`; we filter
        // that out and fall back to `.` so the pathspec computation has a
        // real directory to anchor against.
        assert_eq!(scope_dir(Path::new("ghost.txt")), PathBuf::from("."));
    }

    #[test]
    fn normalize_existing_falls_back_to_absolute_for_missing_path() {
        use super::normalize_existing;
        // canonicalize fails because the path doesn't exist, but
        // `std::path::absolute` succeeds lexically.
        let result = normalize_existing(Path::new("/tmp/freshl-definitely-missing-12345"));
        assert_eq!(
            result,
            Some(PathBuf::from("/tmp/freshl-definitely-missing-12345"))
        );
    }

    #[test]
    fn pathspec_for_returns_empty_when_scope_equals_workdir() {
        use super::pathspec_for;
        let workdir = PathBuf::from("/repo");
        let patterns = pathspec_for(&workdir, &workdir).unwrap();
        assert!(patterns.is_empty());
    }

    #[test]
    fn pathspec_for_returns_relative_pattern_for_subdir() {
        use super::pathspec_for;
        let scope = PathBuf::from("/repo/src");
        let workdir = PathBuf::from("/repo");
        let patterns = pathspec_for(&scope, &workdir).unwrap();
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].as_slice(), b"src");
    }

    #[test]
    fn pathspec_for_returns_none_for_unrelated_paths() {
        use super::pathspec_for;
        let scope = PathBuf::from("/elsewhere");
        let workdir = PathBuf::from("/repo");
        assert!(pathspec_for(&scope, &workdir).is_none());
    }

    #[test]
    fn rename_in_worktree_status_uses_worktree_column() {
        // Worktree-only renames should mark the worktree column so they don't
        // visually masquerade as staged changes.
        assert_eq!(PorcelainCode::RENAMED_WORKTREE.index, ' ');
        assert_eq!(PorcelainCode::RENAMED_WORKTREE.worktree, '→');
        assert_eq!(PorcelainCode::COPIED_WORKTREE.index, ' ');
        assert_eq!(PorcelainCode::COPIED_WORKTREE.worktree, '⇉');
    }

    #[test]
    fn dirty_ancestors_includes_parents_of_modified_file() {
        let (_g, dir) = fresh_repo();
        let nested = dir.path().join("a/b/c");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("file"), b"orig\n").unwrap();
        std::fs::create_dir(dir.path().join("sibling")).unwrap();
        std::fs::write(dir.path().join("sibling/other"), b"x").unwrap();
        run_git(dir.path(), &["add", "."]);
        run_git(dir.path(), &["commit", "-q", "-m", "init"]);
        std::fs::write(nested.join("file"), b"modified\n").unwrap();
        let snap = snapshot_for(dir.path());
        assert!(snap.has_dirty_descendants(&dir.path().join("a")));
        assert!(snap.has_dirty_descendants(&dir.path().join("a/b")));
        assert!(snap.has_dirty_descendants(&dir.path().join("a/b/c")));
        assert!(!snap.has_dirty_descendants(&dir.path().join("sibling")));
    }

    #[test]
    fn dirty_ancestors_excludes_ignored_descendants() {
        let (_g, dir) = fresh_repo();
        let sub = dir.path().join("dir");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("tracked"), b"x").unwrap();
        std::fs::write(dir.path().join(".gitignore"), b"dir/hidden\n").unwrap();
        run_git(dir.path(), &["add", "."]);
        run_git(dir.path(), &["commit", "-q", "-m", "init"]);
        std::fs::write(sub.join("hidden"), b"x").unwrap();
        let snap = snapshot_for(dir.path());
        // `dir/hidden` is IGNORED; its ancestor `dir` must not be flagged.
        assert!(!snap.has_dirty_descendants(&sub));
    }

    #[test]
    fn dirty_ancestors_includes_untracked_in_tracked_dir() {
        let (_g, dir) = fresh_repo();
        let sub = dir.path().join("dir");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("tracked"), b"x").unwrap();
        run_git(dir.path(), &["add", "."]);
        run_git(dir.path(), &["commit", "-q", "-m", "init"]);
        std::fs::write(sub.join("new"), b"x").unwrap();
        let snap = snapshot_for(dir.path());
        assert!(snap.has_dirty_descendants(&sub));
    }

    #[test]
    fn dirty_ancestors_does_not_flag_clean_repo() {
        let (_g, dir) = fresh_repo();
        std::fs::write(dir.path().join("file"), b"x").unwrap();
        run_git(dir.path(), &["add", "."]);
        run_git(dir.path(), &["commit", "-q", "-m", "init"]);
        let snap = snapshot_for(dir.path());
        assert!(!snap.has_dirty_descendants(dir.path()));
        assert!(!snap.has_dirty_descendants(&dir.path().join("file")));
    }

    #[test]
    fn has_dirty_descendants_false_outside_root() {
        let snap = Snapshot {
            root: PathBuf::from("/repo"),
            ..Default::default()
        };
        assert!(!snap.has_dirty_descendants(Path::new("/elsewhere/dir")));
    }

    #[test]
    fn display_code_for_returns_clean_outside_root() {
        let snap = Snapshot {
            root: PathBuf::from("/repo"),
            ..Default::default()
        };
        assert_eq!(
            snap.display_code_for(Path::new("/elsewhere/dir"), true),
            PorcelainCode::CLEAN,
        );
    }

    #[test]
    fn has_dirty_descendants_true_for_root_with_dirty_subtree() {
        // `freshl -d <root>` needs the root row itself to flag a dirty tree.
        let (_g, dir) = fresh_repo();
        std::fs::write(dir.path().join("a"), b"x").unwrap();
        run_git(dir.path(), &["add", "."]);
        run_git(dir.path(), &["commit", "-q", "-m", "init"]);
        std::fs::write(dir.path().join("a"), b"changed").unwrap();
        let snap = snapshot_for(dir.path());
        assert!(snap.has_dirty_descendants(dir.path()));
        assert_eq!(
            snap.display_code_for(dir.path(), true),
            PorcelainCode::DIRTY_SUBTREE,
        );
    }
}
