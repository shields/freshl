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

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use gix::bstr::BStr;

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

/// Per-repository view used for status rendering.
///
/// The lookup invariant: `PorcelainCode::CLEAN` is returned *only* when the
/// path has positive proof of being in the index. Anything else is
/// `IGNORED`, `UNTRACKED`, or one of the change codes — never `CLEAN` by
/// default. Achieving that requires three sources of truth:
///   - `statuses`: what the status walk reported (changes + collapsed
///     untracked/ignored dirs).
///   - `tracked_prefixes`: every indexed path *and* all of its ancestor
///     directories, so a directory row counts as "tracked" iff at least one
///     of its descendants is in the index.
///   - `excludes`: a gix exclude stack consulted on demand for paths that
///     are neither in `statuses` nor in `tracked_prefixes`.
///
/// `repo` is held because the detached exclude stack's `at_path` call
/// requires the repository's object database to resolve in-tree
/// `.gitignore` files.
pub struct Snapshot {
    pub root: PathBuf,
    pub statuses: HashMap<PathBuf, PorcelainCode>,
    dirty_ancestors: HashSet<PathBuf>,
    tracked_prefixes: HashSet<PathBuf>,
    /// Index entries with `Mode::COMMIT` — submodule gitlinks. The parent
    /// repo doesn't track contents under these paths; the submodule does.
    /// Used by `lookup_rel` to classify submodule descendants as CLEAN
    /// instead of UNTRACKED.
    submodule_roots: HashSet<PathBuf>,
    repo: gix::Repository,
    excludes: RefCell<gix::worktree::Stack>,
}

impl std::fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `gix::worktree::Stack` doesn't impl Debug; render a compact view.
        f.debug_struct("Snapshot")
            .field("root", &self.root)
            .field("statuses", &self.statuses)
            .field("dirty_ancestors", &self.dirty_ancestors)
            .field("tracked_prefixes_len", &self.tracked_prefixes.len())
            .field("submodule_roots", &self.submodule_roots)
            .finish_non_exhaustive()
    }
}

impl Snapshot {
    /// Resolve `path` to its `PorcelainCode`.
    ///
    /// Unknown paths are classified as `UNTRACKED` (or `IGNORED` if the
    /// exclude stack matches) — never `CLEAN`. `CLEAN` is reserved for
    /// paths the index has positive proof of (see `lookup_rel`).
    /// `PorcelainCode::BLANK` is returned for paths outside the snapshot's
    /// workdir (so cross-repo arguments render no glyph instead of a spurious
    /// `?`) and for the repo's own `.git` directory and anything beneath it
    /// (the git column has nothing meaningful to say about its own metadata).
    ///
    /// Filesystem kind probing is deferred to the cold path — only the
    /// exclude check needs it, so `lookup`'s common case (path is in
    /// `statuses`/`tracked_prefixes`) avoids any `stat` syscall.
    ///
    /// Path normalisation tries lexical absolutisation first (cheap, preserves
    /// symlink identity for entries that are themselves symlinks). If the
    /// lexical result fails to land inside `self.root` or contains unresolved
    /// `..` components, it falls back to [`std::fs::canonicalize`] — that
    /// covers symlinked workdirs and `freshl ..` style paths.
    #[must_use]
    pub fn lookup(&self, path: &Path) -> PorcelainCode {
        let Some(rel) = self.relativize(path) else {
            return PorcelainCode::BLANK;
        };
        // `symlink_metadata` (lstat) so a symlink itself reports `is_dir=false`,
        // matching git's "symlinks to directories are not considered directories
        // for the purpose of matching" rule. `metadata` would follow the link
        // and cause directory-only patterns (e.g. `vendor/`) to match symlinks.
        self.lookup_rel(&rel, || {
            std::fs::symlink_metadata(path).ok().map(|m| m.is_dir())
        })
    }

    /// Resolve `rel` to its `PorcelainCode`. `kind_resolver` is called at most
    /// once and only when the exclude check needs to know whether `rel` is a
    /// directory (i.e. only on the cold path).
    fn lookup_rel<F: FnOnce() -> Option<bool>>(
        &self,
        rel: &Path,
        kind_resolver: F,
    ) -> PorcelainCode {
        // The repo's own metadata at `.git` isn't source-controlled (a
        // directory in a primary worktree, a linkfile in a secondary
        // worktree). `Path::starts_with` matches component-wise, so this
        // catches `.git` and `.git/HEAD` but not sibling names like
        // `.gitignore`, and not a nested `subdir/.git` — the latter is
        // either a submodule linkfile (resolved to CLEAN below because
        // its `subdir` ancestor is in `submodule_roots`) or a stray file
        // (falls through to UNTRACKED).
        if rel.starts_with(".git") {
            return PorcelainCode::BLANK;
        }
        if let Some(code) = self.statuses.get(rel).copied() {
            return code;
        }
        for ancestor in iter_ancestors(rel) {
            if let Some(code) = self.statuses.get(ancestor).copied()
                && (code == PorcelainCode::UNTRACKED || code == PorcelainCode::IGNORED)
            {
                return code;
            }
            // A submodule gitlink ancestor means the parent repo delegates
            // this subtree to the submodule — classify descendants as CLEAN
            // so the listing doesn't drown in spurious `?` glyphs.
            if self.submodule_roots.contains(ancestor) {
                return PorcelainCode::CLEAN;
            }
        }
        // The status walk had no entry for this path. Resolve by positive
        // proof: CLEAN requires the path (or a descendant) to be in the
        // index; otherwise it's IGNORED (matches an exclude rule) or
        // UNTRACKED.
        if self.tracked_prefixes.contains(rel) {
            return PorcelainCode::CLEAN;
        }
        if self.path_is_excluded(rel, kind_resolver()) {
            PorcelainCode::IGNORED
        } else {
            PorcelainCode::UNTRACKED
        }
    }

    fn path_is_excluded(&self, rel: &Path, is_directory: Option<bool>) -> bool {
        // Unknown kind (e.g. lookup on a path that doesn't exist on disk):
        // try both modes. Patterns either gate on `MUST_BE_DIR` or don't,
        // so checking both is a strict superset — no risk of a false hit.
        is_directory.map_or_else(
            || self.check_excluded(rel, true) || self.check_excluded(rel, false),
            |d| self.check_excluded(rel, d),
        )
    }

    fn check_excluded(&self, rel: &Path, is_directory: bool) -> bool {
        let mode = if is_directory {
            gix::index::entry::Mode::DIR
        } else {
            gix::index::entry::Mode::FILE
        };
        // `is_ok_and` short-circuits an `at_path` error to "not excluded".
        // The exclude stack reads `.gitignore` files lazily; a transient
        // I/O error mid-walk shouldn't crash the lookup.
        self.excludes
            .borrow_mut()
            .at_path(rel, Some(mode), &self.repo.objects)
            .is_ok_and(|p| p.is_excluded())
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
    /// Single path-normalisation for both checks. Outside-root paths return
    /// [`PorcelainCode::BLANK`] (no glyph), matching [`Self::lookup`].
    #[must_use]
    pub fn display_code_for(&self, path: &Path, is_directory: bool) -> PorcelainCode {
        let Some(rel) = self.relativize(path) else {
            return PorcelainCode::BLANK;
        };
        let direct = self.lookup_rel(&rel, || Some(is_directory));
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

/// Caches snapshots keyed by canonical repository workdir.
///
/// The status walk is always full-repo (empty pathspec) — pathspec-limited
/// walks descend into gitignored subtrees in gix (orders of magnitude
/// slower) for no gain, since `Snapshot::lookup` resolves any path inside
/// the workdir without needing the walk to have been scoped. Multiple
/// targets inside the same repo therefore share a single snapshot.
///
/// `by_scope` is a separate negative/positive cache mapping each scope
/// directory to the canonical workdir it resolves to (or `None` if no
/// repository was found). It saves a second `gix::discover` walk when the
/// same scope is requested twice and caches negative results so `freshl *`
/// in a non-git directory doesn't re-traverse the filesystem per target.
#[derive(Debug, Default)]
pub struct SnapshotCache {
    by_scope: HashMap<PathBuf, Option<PathBuf>>,
    by_workdir: HashMap<PathBuf, Option<Snapshot>>,
}

impl SnapshotCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn for_target(&mut self, target: &Path) -> Option<&Snapshot> {
        let scope = normalize_existing(&scope_dir(target))?;
        if !self.by_scope.contains_key(&scope) {
            self.populate_scope(&scope);
        }
        let workdir = self.by_scope.get(&scope)?.clone()?;
        self.by_workdir.get(&workdir)?.as_ref()
    }

    fn populate_scope(&mut self, scope: &Path) {
        // `and_then` chains the two failure modes — no repository, or a
        // bare repo with no workdir — into a single `None` so the negative
        // cache only needs one insertion point.
        let resolved = gix::discover(scope).ok().and_then(|repo| {
            repo.workdir()
                .and_then(normalize_existing)
                .map(|wd| (repo, wd))
        });
        let Some((repo, workdir)) = resolved else {
            self.by_scope.insert(scope.to_path_buf(), None);
            return;
        };
        self.by_scope
            .insert(scope.to_path_buf(), Some(workdir.clone()));
        if self.by_workdir.contains_key(&workdir) {
            return;
        }
        let snap = build_snapshot(repo, workdir.clone());
        self.by_workdir.insert(workdir, snap);
    }
}

fn build_snapshot(repo: gix::Repository, workdir: PathBuf) -> Option<Snapshot> {
    let statuses = collect_statuses(&repo).unwrap_or_default();
    assemble_snapshot(repo, workdir, statuses)
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

fn rela_to_pathbuf(b: &BStr) -> PathBuf {
    // gix paths are raw bytes; on Unix go through OsStr directly so non-UTF-8
    // names survive (to_os_str_lossy would replace invalid sequences with U+FFFD).
    PathBuf::from(OsStr::from_bytes(b.as_ref()))
}

#[must_use]
pub fn discover(start: &Path) -> Option<Snapshot> {
    let repo = gix::discover(start).ok()?;
    let workdir = normalize_existing(repo.workdir()?)?;
    let statuses = collect_statuses(&repo).unwrap_or_default();
    assemble_snapshot(repo, workdir, statuses)
}

fn assemble_snapshot(
    repo: gix::Repository,
    workdir: PathBuf,
    statuses: HashMap<PathBuf, PorcelainCode>,
) -> Option<Snapshot> {
    // `index_or_empty` synthesises an empty index on a fresh `git init` (no
    // index file on disk yet); we only get `Err` from a corrupt index file,
    // in which case Snapshot construction has to fail — without a usable
    // index there's no way to honour the CLEAN-requires-proof invariant.
    let index = repo.index_or_empty().ok()?;
    let excludes = repo
        .excludes(
            &index,
            None,
            gix::worktree::stack::state::ignore::Source::WorktreeThenIdMappingIfNotSkipped,
        )
        .ok()?
        .detach();
    let dirty_ancestors = compute_dirty_ancestors(&statuses);
    let (tracked_prefixes, submodule_roots) = collect_index_prefixes(&index);
    Some(Snapshot {
        root: workdir,
        statuses,
        dirty_ancestors,
        tracked_prefixes,
        submodule_roots,
        repo,
        excludes: RefCell::new(excludes),
    })
}

fn collect_index_prefixes(index: &gix::index::File) -> (HashSet<PathBuf>, HashSet<PathBuf>) {
    let mut prefixes: HashSet<PathBuf> = HashSet::new();
    let mut submodules: HashSet<PathBuf> = HashSet::new();
    // The repo root itself counts as "has tracked content" so a directory
    // row at the root doesn't fall through to UNTRACKED. The empty path is
    // always present so the root row gets CLEAN treatment in an empty repo
    // too.
    prefixes.insert(PathBuf::new());
    for entry in index.entries() {
        let path = rela_to_pathbuf(entry.path(index));
        if entry.mode.contains(gix::index::entry::Mode::COMMIT) {
            submodules.insert(path.clone());
        }
        for ancestor in path.ancestors() {
            if !prefixes.insert(ancestor.to_path_buf()) {
                break;
            }
        }
    }
    (prefixes, submodules)
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

    // Empty pathspec: always walk the full repo. A non-empty pathspec
    // forces gix to descend into gitignored subtrees (e.g. `node_modules/`),
    // turning what should be ~10 ms into ~1.6 s.
    let iter = platform.into_iter(Vec::new())?;
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
            source,
            dirwalk_entry,
            copy,
            ..
        } => {
            let path = rela_to_pathbuf(dirwalk_entry.rela_path.as_ref());
            let code = rewrite_code(*copy);
            let prev = out.get(&path).copied();
            out.insert(path, merge(prev, code));
            // A rename (not a copy) takes the source out of the worktree;
            // the index still has the source path, so `tracked_prefixes`
            // would otherwise claim CLEAN for a file that's gone. Mark the
            // source as DELETED_WORKTREE explicitly. A copy keeps the
            // source on disk, so there's nothing to record for it.
            if !*copy
                && let gix::status::index_worktree::RewriteSource::RewriteFromIndex {
                    source_rela_path,
                    ..
                } = source
            {
                let source_path = rela_to_pathbuf(source_rela_path.as_ref());
                let prev = out.get(&source_path).copied();
                out.insert(source_path, merge(prev, PorcelainCode::DELETED_WORKTREE));
            }
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
#[expect(
    clippy::significant_drop_tightening,
    reason = "TestRepo intentionally holds GIT_LOCK and TempDir for the entire test scope so parallel git subprocesses can't share index state via environment"
)]
mod tests {
    use super::{PorcelainCode, Snapshot, SnapshotCache, discover, merge};
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

    /// Fluent test fixture: a freshly-initialised git repo in a tempdir,
    /// holding `GIT_LOCK` for its lifetime so concurrent tests don't
    /// interleave git subprocesses or share index state via env vars.
    ///
    /// Methods return `&Self` so common setup reads like a builder:
    ///   `r.write("a", b"x").commit(&["a"], "msg").write("a", b"y");`
    struct TestRepo {
        _lock: MutexGuard<'static, ()>,
        dir: TempDir,
    }

    impl TestRepo {
        fn new() -> Self {
            // `unwrap_or_else(into_inner)` keeps a failing test from
            // cascading into ~40 PoisonError failures and masking the real
            // root cause. The lock's job is serialisation; a previous panic
            // doesn't make later acquisitions unsafe.
            let lock = GIT_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let dir = tempfile::tempdir().unwrap();
            run_git(dir.path(), &["init", "-q", "-b", "main"]);
            run_git(dir.path(), &["config", "user.email", "t@example.invalid"]);
            run_git(dir.path(), &["config", "user.name", "t"]);
            Self { _lock: lock, dir }
        }

        fn root(&self) -> &Path {
            self.dir.path()
        }

        fn write(&self, rel: &str, content: &[u8]) -> &Self {
            let p = self.dir.path().join(rel);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(p, content).unwrap();
            self
        }

        fn create_dir(&self, rel: &str) -> &Self {
            std::fs::create_dir_all(self.dir.path().join(rel)).unwrap();
            self
        }

        fn remove_file(&self, rel: &str) -> &Self {
            std::fs::remove_file(self.dir.path().join(rel)).unwrap();
            self
        }

        fn rename(&self, from: &str, to: &str) -> &Self {
            std::fs::rename(self.dir.path().join(from), self.dir.path().join(to)).unwrap();
            self
        }

        fn symlink(&self, target: impl AsRef<Path>, rel: &str) -> &Self {
            std::os::unix::fs::symlink(target, self.dir.path().join(rel)).unwrap();
            self
        }

        fn git(&self, args: &[&str]) -> &Self {
            run_git(self.dir.path(), args);
            self
        }

        /// `git add <add_args>` followed by `git commit -m <msg>`. Pass
        /// `&["."]` to stage everything.
        fn commit(&self, add_args: &[&str], msg: &str) -> &Self {
            let mut add: Vec<&str> = vec!["add"];
            add.extend_from_slice(add_args);
            self.git(&add).git(&["commit", "-q", "-m", msg])
        }

        fn snapshot(&self) -> Snapshot {
            discover(self.dir.path()).expect("repo present")
        }
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
    fn lookup_returns_untracked_for_unknown_path_inside_root() {
        // An empty repo with no entries: nothing is tracked, nothing has a
        // status, nothing matches an ignore rule. A path INSIDE the repo
        // root must resolve to UNTRACKED — never CLEAN.
        let r = TestRepo::new();
        assert_eq!(
            r.snapshot().lookup(&r.root().join("ghost")),
            PorcelainCode::UNTRACKED,
        );
    }

    #[test]
    fn lookup_returns_blank_for_path_outside_root() {
        // Outside the snapshot's workdir, lookup has nothing to say — return
        // BLANK so the git column stays empty (no spurious `?` glyph).
        let r = TestRepo::new();
        let sibling = tempfile::tempdir().unwrap();
        assert_eq!(
            r.snapshot().lookup(&sibling.path().join("elsewhere")),
            PorcelainCode::BLANK,
        );
    }

    #[test]
    fn lookup_uses_status_walk_entry_for_known_path() {
        // Stage a tracked-then-modified file: the status walk emits
        // MODIFIED_WORKTREE, which is distinguishable from both the CLEAN
        // outcome (would mean tracked-fine) and the UNTRACKED outcome
        // (would mean the per-path classifier ran). So this test pins the
        // statuses precedence — a regression that reorders statuses vs
        // tracked_prefixes can no longer pass by accident.
        let r = TestRepo::new();
        r.write("a", b"one\n").commit(&["a"], "init");
        r.write("a", b"two\n");
        assert_eq!(
            status_at(&r.snapshot(), "a"),
            PorcelainCode::MODIFIED_WORKTREE,
        );
    }

    #[test]
    fn is_ignored_only_true_for_ignored_code() {
        let r = TestRepo::new();
        r.write(".gitignore", b"ig\n")
            .write("ig", b"x")
            .write("un", b"x");
        let snap = r.snapshot();
        assert!(snap.is_ignored(&r.root().join("ig")));
        assert!(!snap.is_ignored(&r.root().join("un")));
        assert!(!snap.is_ignored(&r.root().join("missing")));
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
        let r = TestRepo::new();
        r.write("new", b"x");
        assert_eq!(status_at(&r.snapshot(), "new"), PorcelainCode::UNTRACKED);
    }

    #[test]
    fn discover_reports_ignored_file() {
        let r = TestRepo::new();
        r.write(".gitignore", b"hidden\n").write("hidden", b"x");
        assert_eq!(status_at(&r.snapshot(), "hidden"), PorcelainCode::IGNORED);
    }

    #[test]
    fn discover_reports_modified_worktree() {
        let r = TestRepo::new();
        r.write("a", b"hello\n").commit(&["a"], "x");
        r.write("a", b"different\n");
        assert_eq!(
            status_at(&r.snapshot(), "a"),
            PorcelainCode::MODIFIED_WORKTREE,
        );
    }

    #[test]
    fn discover_reports_deleted_worktree() {
        let r = TestRepo::new();
        r.write("b", b"x").commit(&["b"], "x").remove_file("b");
        assert_eq!(
            status_at(&r.snapshot(), "b"),
            PorcelainCode::DELETED_WORKTREE,
        );
    }

    #[test]
    fn discover_reports_staged_addition() {
        // The tree-vs-index diff needs at least one commit on HEAD to compare against.
        let r = TestRepo::new();
        r.write("seed", b"x").commit(&["seed"], "seed");
        r.write("staged", b"hi").git(&["add", "staged"]);
        assert_eq!(status_at(&r.snapshot(), "staged").index, '+');
    }

    #[test]
    fn discover_reports_staged_modification() {
        let r = TestRepo::new();
        r.write("m", b"one\n").commit(&["m"], "m");
        r.write("m", b"two\n").git(&["add", "m"]);
        assert_eq!(status_at(&r.snapshot(), "m").index, '●');
    }

    #[test]
    fn discover_reports_staged_deletion() {
        let r = TestRepo::new();
        r.write("d", b"x")
            .commit(&["d"], "d")
            .git(&["rm", "-q", "d"]);
        assert_eq!(status_at(&r.snapshot(), "d").index, '▽');
    }

    #[test]
    fn discover_reports_rename_in_worktree() {
        // 40 lines so the similarity threshold inside gix's rewrite detector
        // is comfortably above the default 50% — otherwise the rename is
        // reported as an add+delete pair.
        let r = TestRepo::new();
        let body = "line\n".repeat(40);
        r.write("from", body.as_bytes()).commit(&["from"], "from");
        r.rename("from", "to");
        assert_eq!(
            status_at(&r.snapshot(), "to"),
            PorcelainCode::RENAMED_WORKTREE,
        );
    }

    #[test]
    fn discover_reports_staged_rename() {
        let r = TestRepo::new();
        let body = "line\n".repeat(40);
        r.write("from", body.as_bytes()).commit(&["from"], "from");
        r.git(&["mv", "from", "to"]);
        assert_eq!(status_at(&r.snapshot(), "to").index, '→');
    }

    #[test]
    fn discover_reports_unmerged_conflict() {
        let r = TestRepo::new();
        r.write("c", b"base\n").commit(&["c"], "base");
        r.git(&["checkout", "-q", "-b", "other"]);
        r.write("c", b"other\n")
            .git(&["commit", "-q", "-am", "other"]);
        r.git(&["checkout", "-q", "main"]);
        r.write("c", b"main\n")
            .git(&["commit", "-q", "-am", "main"]);
        // git merge exits non-zero on conflict; ignore its status.
        let _ = Command::new("git")
            .arg("-C")
            .arg(r.root())
            .args(["merge", "--no-edit", "-q", "other"])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("HOME", r.root())
            .status()
            .unwrap();
        assert_eq!(status_at(&r.snapshot(), "c"), PorcelainCode::UNMERGED);
    }

    #[test]
    fn discover_reports_type_change() {
        let r = TestRepo::new();
        r.write("t", b"file").commit(&["t"], "t").remove_file("t");
        r.symlink("anything", "t");
        assert_eq!(
            status_at(&r.snapshot(), "t"),
            PorcelainCode::TYPE_CHANGE_WORKTREE,
        );
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
        // Whole-untracked subdirectories are emitted as one collapsed entry
        // by gix; descendants inherit UNTRACKED via the ancestor walk.
        let r = TestRepo::new();
        r.write("dir/file", b"x").write("dir/deeper/file", b"x");
        let snap = r.snapshot();
        assert_eq!(
            snap.lookup(&r.root().join("dir/file")),
            PorcelainCode::UNTRACKED,
        );
        assert_eq!(
            snap.lookup(&r.root().join("dir/deeper/file")),
            PorcelainCode::UNTRACKED,
        );
        // A path outside the collapsed directory and not in the index falls
        // through to UNTRACKED as well — never CLEAN.
        assert_eq!(
            snap.lookup(&r.root().join("other")),
            PorcelainCode::UNTRACKED,
        );
    }

    #[test]
    fn lookup_inherits_ignored_from_ancestor() {
        // A whole-ignored directory is emitted as one collapsed entry; a
        // child path inherits IGNORED via the ancestor walk.
        let r = TestRepo::new();
        r.write(".gitignore", b"ig/\n").write("ig/inside", b"x");
        let snap = r.snapshot();
        assert_eq!(
            snap.lookup(&r.root().join("ig/inside")),
            PorcelainCode::IGNORED,
        );
    }

    #[test]
    fn lookup_does_not_inherit_modified_from_ancestor() {
        // A modified file's status must not bleed onto something that looks
        // like its child (nonsense for regular files; possible for
        // type-change cases).
        let r = TestRepo::new();
        r.write("mod", b"orig\n").commit(&["mod"], "init");
        r.write("mod", b"changed\n");
        let snap = r.snapshot();
        // `mod/inside` doesn't exist; the MODIFIED status of `mod` must
        // not be inherited by phantom descendants.
        assert_ne!(
            snap.lookup(&r.root().join("mod/inside")),
            PorcelainCode::MODIFIED_WORKTREE,
        );
    }

    #[test]
    fn lookup_resolves_dotdot_via_canonicalize_fallback() {
        // The lexical strip yields `sub/../file`; the canonicalize fallback
        // simplifies that to `file` so the lookup matches.
        let r = TestRepo::new();
        r.write("file", b"x").create_dir("sub");
        let canonical_root = std::fs::canonicalize(r.root()).unwrap();
        let weird = canonical_root.join("sub").join("..").join("file");
        assert_eq!(r.snapshot().lookup(&weird), PorcelainCode::UNTRACKED);
    }

    #[test]
    fn lookup_returns_blank_when_canonicalize_lands_outside_root() {
        // A real path that lands outside the snapshot's workdir: `relativize`
        // yields None → lookup returns BLANK so the row renders no glyph.
        let r = TestRepo::new();
        let snap = r.snapshot();
        let sibling = tempfile::tempdir().unwrap();
        std::fs::write(sibling.path().join("file"), b"x").unwrap();
        assert_eq!(
            snap.lookup(&sibling.path().join("file")),
            PorcelainCode::BLANK,
        );
    }

    #[test]
    fn lookup_returns_blank_when_canonicalize_fails() {
        // A `..` path with no real on-disk components forces both lexical
        // strip and canonicalize to fail → relativize yields None → BLANK.
        let r = TestRepo::new();
        assert_eq!(
            r.snapshot()
                .lookup(&r.root().join("missing/../also-missing")),
            PorcelainCode::BLANK,
        );
    }

    #[test]
    fn lookup_preserves_leaf_symlink_via_parent_canonicalisation() {
        // The canonicalize fallback must NOT dereference a leaf symlink — if
        // it did, a symlinked entry would look up under its target's path
        // instead of its own. Set up a TYPE_CHANGE_WORKTREE for `entry` (a
        // committed regular file replaced by a symlink) so the test has a
        // discriminating status code: following the symlink to `/dev/null`
        // would yield a different lookup result entirely (BLANK, since
        // `/dev/null` is outside the snapshot root), so any future
        // regression that follows the leaf symlink will fail loudly.
        let r = TestRepo::new();
        r.write("entry", b"originally a file")
            .commit(&["entry"], "init");
        r.remove_file("entry").symlink("/dev/null", "entry");
        let canonical = std::fs::canonicalize(r.root()).unwrap();
        // Put the directory symlink in a separate tempdir so its path can't
        // share a prefix with `canonical`. On platforms where the tempdir
        // is already canonical (Linux), the lexical strip_prefix would
        // otherwise succeed and yield `via_link/entry`, bypassing the
        // parent-canonicalisation fallback this test exists to cover.
        let link_parent = tempfile::tempdir().unwrap();
        let link_dir = link_parent.path().join("via_link");
        std::os::unix::fs::symlink(&canonical, &link_dir).unwrap();

        let snap = r.snapshot();
        // Reaching `entry` via the directory-symlink path must resolve to
        // TYPE_CHANGE_WORKTREE — the status of the symlink itself, not the
        // status of `/dev/null`.
        assert_eq!(
            snap.lookup(&link_dir.join("entry")),
            PorcelainCode::TYPE_CHANGE_WORKTREE,
        );
    }

    #[test]
    fn lookup_handles_single_component_relative_path() {
        // `Path::new("solo").parent()` is `Some("")`; the fallback must
        // substitute "." so canonicalize doesn't choke on the empty path.
        // CWD during tests is the package root, not under the tempdir, so
        // the path can't be relativized → outside-root → BLANK.
        let r = TestRepo::new();
        assert_eq!(r.snapshot().lookup(Path::new("solo")), PorcelainCode::BLANK);
    }

    #[test]
    fn lookup_handles_path_with_no_file_name() {
        // `Path::new("..").file_name()` is `None`. The fallback must
        // canonicalise the whole path rather than panic via `?`. The parent
        // of the tempdir is outside the root → BLANK.
        let r = TestRepo::new();
        assert_eq!(r.snapshot().lookup(Path::new("..")), PorcelainCode::BLANK);
    }

    #[test]
    fn lookup_returns_blank_for_path_in_unrelated_tempdir() {
        let r = TestRepo::new();
        let snap = r.snapshot();
        let sibling = tempfile::tempdir().unwrap();
        assert_eq!(
            snap.lookup(&sibling.path().join("file")),
            PorcelainCode::BLANK,
        );
    }

    #[test]
    fn snapshot_cache_reuses_entry_for_same_target() {
        let r = TestRepo::new();
        let mut cache = SnapshotCache::new();
        let first_ptr: *const Snapshot = cache.for_target(r.root()).unwrap();
        let second_ptr: *const Snapshot = cache.for_target(r.root()).unwrap();
        assert!(std::ptr::eq(first_ptr, second_ptr));
    }

    #[test]
    fn snapshot_cache_shares_snapshot_across_subdirs_in_same_repo() {
        // Two subdirectories of the same repo must resolve to the *same*
        // snapshot — one full-repo walk, not one per scope. Sharing also
        // ensures the snapshot's `statuses` covers both subtrees so a
        // lookup outside the originally-requested scope still works.
        let r = TestRepo::new();
        r.write("a/tracked", b"x")
            .write("b/tracked", b"x")
            .commit(&["."], "init");
        r.write("a/only_a", b"x").write("b/only_b", b"x");

        let mut cache = SnapshotCache::new();
        let first_ptr: *const Snapshot = cache.for_target(&r.root().join("a")).unwrap();
        let second_ptr: *const Snapshot = cache.for_target(&r.root().join("b")).unwrap();
        assert!(std::ptr::eq(first_ptr, second_ptr));
        let statuses: Vec<_> = cache
            .for_target(&r.root().join("a"))
            .unwrap()
            .statuses
            .keys()
            .cloned()
            .collect();
        assert!(statuses.iter().any(|p| p.ends_with("only_a")));
        assert!(statuses.iter().any(|p| p.ends_with("only_b")));
    }

    #[test]
    fn snapshot_cache_returns_none_outside_repo() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = SnapshotCache::new();
        assert!(cache.for_target(dir.path()).is_none());
    }

    #[test]
    fn snapshot_cache_returns_none_when_build_fails() {
        // Cache the negative result of `build_snapshot` returning `None`
        // (corrupt index here, same trigger as `discover_returns_none_for_corrupt_index`).
        // The workdir is still recorded in `by_scope`/`by_workdir` so a
        // second lookup is free instead of re-attempting the failed build.
        let r = TestRepo::new();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"DIRC");
        bytes.extend_from_slice(&0x99_u32.to_be_bytes());
        bytes.extend_from_slice(&0_u32.to_be_bytes());
        bytes.extend_from_slice(&[0u8; 20]);
        std::fs::write(r.root().join(".git/index"), &bytes).unwrap();
        let mut cache = SnapshotCache::new();
        assert!(cache.for_target(r.root()).is_none());
        assert!(cache.for_target(r.root()).is_none());
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
        assert!(cache.by_scope[&canon].is_none());
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
        let r = TestRepo::new();
        r.write("a/b/c/file", b"orig\n")
            .write("sibling/other", b"x")
            .commit(&["."], "init");
        r.write("a/b/c/file", b"modified\n");
        let snap = r.snapshot();
        assert!(snap.has_dirty_descendants(&r.root().join("a")));
        assert!(snap.has_dirty_descendants(&r.root().join("a/b")));
        assert!(snap.has_dirty_descendants(&r.root().join("a/b/c")));
        assert!(!snap.has_dirty_descendants(&r.root().join("sibling")));
    }

    #[test]
    fn dirty_ancestors_excludes_ignored_descendants() {
        let r = TestRepo::new();
        r.write("dir/tracked", b"x")
            .write(".gitignore", b"dir/hidden\n")
            .commit(&["."], "init");
        r.write("dir/hidden", b"x");
        let snap = r.snapshot();
        // `dir/hidden` is IGNORED; its ancestor `dir` must not be flagged.
        assert!(!snap.has_dirty_descendants(&r.root().join("dir")));
    }

    #[test]
    fn dirty_ancestors_includes_untracked_in_tracked_dir() {
        let r = TestRepo::new();
        r.write("dir/tracked", b"x").commit(&["."], "init");
        r.write("dir/new", b"x");
        let snap = r.snapshot();
        assert!(snap.has_dirty_descendants(&r.root().join("dir")));
    }

    #[test]
    fn dirty_ancestors_does_not_flag_clean_repo() {
        let r = TestRepo::new();
        r.write("file", b"x").commit(&["."], "init");
        let snap = r.snapshot();
        assert!(!snap.has_dirty_descendants(r.root()));
        assert!(!snap.has_dirty_descendants(&r.root().join("file")));
    }

    #[test]
    fn has_dirty_descendants_false_outside_root() {
        let r = TestRepo::new();
        let snap = r.snapshot();
        let sibling = tempfile::tempdir().unwrap();
        assert!(!snap.has_dirty_descendants(&sibling.path().join("dir")));
    }

    #[test]
    fn display_code_for_returns_blank_outside_root() {
        // Outside-root paths can't be relativized; display_code_for matches
        // lookup's BLANK contract so the git column for a cross-repo
        // argument stays empty rather than rendering a misleading `?`.
        let r = TestRepo::new();
        let snap = r.snapshot();
        let sibling = tempfile::tempdir().unwrap();
        assert_eq!(
            snap.display_code_for(&sibling.path().join("dir"), true),
            PorcelainCode::BLANK,
        );
    }

    #[test]
    fn has_dirty_descendants_true_for_root_with_dirty_subtree() {
        // `freshl -d <root>` needs the root row itself to flag a dirty tree.
        let r = TestRepo::new();
        r.write("a", b"x").commit(&["."], "init");
        r.write("a", b"changed");
        let snap = r.snapshot();
        assert!(snap.has_dirty_descendants(r.root()));
        assert_eq!(
            snap.display_code_for(r.root(), true),
            PorcelainCode::DIRTY_SUBTREE,
        );
    }

    // ---------- Regression tests for the CLEAN-requires-proof invariant ----

    #[test]
    fn lookup_marks_ignored_dir_when_scope_is_subdir() {
        // The original bug: listing `backend/` (a subdir of the repo) walked
        // status with pathspec `["backend"]`, and gix didn't emit
        // `backend/node_modules/` as IGNORED at the pathspec boundary. The
        // fix consults the exclude stack per-path instead of relying solely
        // on the walk's emissions.
        let r = TestRepo::new();
        r.write(".gitignore", b"node_modules/\n")
            .write("backend/tracked", b"x")
            .commit(&["."], "init");
        r.write("backend/node_modules/anything", b"x");
        let mut cache = SnapshotCache::new();
        let snap = cache
            .for_target(&r.root().join("backend"))
            .expect("repo present");
        assert_eq!(
            snap.display_code_for(&r.root().join("backend/node_modules"), true),
            PorcelainCode::IGNORED,
        );
    }

    #[test]
    fn lookup_marks_untracked_dir_when_scope_is_subdir() {
        // Symmetric regression: a brand-new directory under a subdir scope
        // must resolve to UNTRACKED, not CLEAN.
        let r = TestRepo::new();
        r.write("backend/tracked", b"x").commit(&["."], "init");
        r.write("backend/brand_new/anything", b"x");
        let mut cache = SnapshotCache::new();
        let snap = cache
            .for_target(&r.root().join("backend"))
            .expect("repo present");
        assert_eq!(
            snap.display_code_for(&r.root().join("backend/brand_new"), true),
            PorcelainCode::UNTRACKED,
        );
    }

    #[test]
    fn lookup_marks_clean_only_when_path_in_index() {
        // CLEAN requires positive proof of being in the index. "No change
        // reported" and "exists on disk" don't qualify.
        let r = TestRepo::new();
        r.write("backend/tracked", b"x").commit(&["."], "init");
        let mut cache = SnapshotCache::new();
        let snap = cache
            .for_target(&r.root().join("backend"))
            .expect("repo present");
        assert_eq!(
            snap.lookup(&r.root().join("backend/tracked")),
            PorcelainCode::CLEAN,
        );
        // A path that's not in the index and has no exclude rule must NOT
        // be CLEAN.
        assert_ne!(
            snap.lookup(&r.root().join("backend/never_existed")),
            PorcelainCode::CLEAN,
        );
    }

    #[test]
    fn lookup_marks_brand_new_file_untracked_not_clean() {
        // The core invariant: a brand-new file with no statuses entry and
        // no exclude rule match resolves to UNTRACKED.
        let r = TestRepo::new();
        r.write("seed", b"x").commit(&["."], "init");
        r.write("brand_new", b"x");
        assert_eq!(
            status_at(&r.snapshot(), "brand_new"),
            PorcelainCode::UNTRACKED,
        );
    }

    #[test]
    fn lookup_consults_exclude_stack_for_unwalked_paths() {
        // Paths the status walk didn't see fall through `statuses` and
        // `tracked_prefixes`; the exclude stack is the only thing left to
        // decide IGNORED vs UNTRACKED. Two flavours exercise both
        // `kind_resolver` branches inside `lookup`: missing-on-disk (lstat
        // fails, resolver returns `None`, both DIR and FILE modes are
        // tried) and created-after-the-snapshot (lstat succeeds, resolver
        // returns `Some(false)`, only FILE mode is tried).
        let r = TestRepo::new();
        r.write(".gitignore", b"*.log\n")
            .write("anchor", b"x")
            .commit(&["."], "init");
        let snap = r.snapshot();
        assert_eq!(
            snap.lookup(&r.root().join("missing.log")),
            PorcelainCode::IGNORED,
        );
        r.write("created_after_snapshot.log", b"x");
        assert_eq!(
            snap.lookup(&r.root().join("created_after_snapshot.log")),
            PorcelainCode::IGNORED,
        );
    }

    #[test]
    fn snapshot_debug_emits_non_empty_repr() {
        // The Debug impl is hand-rolled because `gix::worktree::Stack`
        // doesn't impl Debug; this test makes sure it actually compiles
        // and produces a recognisable string for diagnostic output.
        let r = TestRepo::new();
        let snap = r.snapshot();
        let repr = format!("{snap:?}");
        assert!(repr.contains("Snapshot"));
        assert!(repr.contains("tracked_prefixes_len"));
    }

    #[test]
    fn worktree_rename_marks_source_deleted_not_clean() {
        // `mv from to` after committing `from` triggers a single gix
        // Item::Rewrite. The Rewrite arm must record the source as
        // DELETED_WORKTREE — otherwise the source path stays in
        // `tracked_prefixes` (still in the index) and `lookup` would
        // claim CLEAN for a file that no longer exists.
        let r = TestRepo::new();
        // 40 lines so gix's rewrite detector clears its default 50%
        // similarity threshold.
        let body = "line\n".repeat(40);
        r.write("from", body.as_bytes()).commit(&["from"], "from");
        r.rename("from", "to");
        assert_eq!(
            status_at(&r.snapshot(), "from"),
            PorcelainCode::DELETED_WORKTREE,
        );
    }

    #[test]
    fn submodule_contents_resolve_to_clean() {
        // The parent repo only tracks the submodule's gitlink commit, not
        // its contents — but those contents shouldn't render as `?`. With
        // a gitlink in submodule_roots, lookup under the submodule path
        // returns CLEAN (we delegate that subtree to the submodule).
        let r = TestRepo::new();
        // Set up a real inner repo to register as a submodule.
        let inner = tempfile::tempdir().unwrap();
        run_git(inner.path(), &["init", "-q", "-b", "main"]);
        run_git(inner.path(), &["config", "user.email", "t@example.invalid"]);
        run_git(inner.path(), &["config", "user.name", "t"]);
        std::fs::write(inner.path().join("inside"), b"x").unwrap();
        run_git(inner.path(), &["add", "inside"]);
        run_git(inner.path(), &["commit", "-q", "-m", "inner"]);
        // `git submodule add` from a local path; the protocol restriction
        // env var lets file:// urls through in modern git.
        let inner_url = format!("file://{}", inner.path().display());
        let status = Command::new("git")
            .arg("-C")
            .arg(r.root())
            .args(["-c", "protocol.file.allow=always"])
            .args(["submodule", "add", "-q", &inner_url, "submod"])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("HOME", r.root())
            .status()
            .unwrap();
        assert!(status.success(), "git submodule add failed");
        r.commit(&["."], "add submod");
        let snap = r.snapshot();
        assert_eq!(
            snap.lookup(&r.root().join("submod/inside")),
            PorcelainCode::CLEAN,
        );
    }

    #[test]
    fn symlink_to_directory_matching_dir_rule_is_not_ignored() {
        // `man gitignore`: "Symbolic links to directories are not considered
        // directories for the purpose of matching." After the fix,
        // `lookup` uses lstat (so is_dir=false for the symlink itself) and
        // a trailing-slash rule fails to match.
        let r = TestRepo::new();
        r.write(".gitignore", b"vendor/\n")
            .commit(&[".gitignore"], "ig");
        // Symlink target need not exist on disk — gix only consults
        // the link's own kind, not the target.
        r.symlink("/tmp", "vendor");
        let snap = r.snapshot();
        assert_ne!(
            snap.lookup(&r.root().join("vendor")),
            PorcelainCode::IGNORED,
        );
    }

    #[test]
    fn lookup_returns_untracked_when_exclude_stack_at_path_errors() {
        // The exclude stack walks ancestors to push directories. If an
        // ancestor exists as a regular file (not a directory), `at_path`
        // returns an I/O error; `path_is_excluded` swallows it and the
        // lookup falls through to UNTRACKED. Triggering this via a file
        // posing as a parent works regardless of uid (chmod 0o000 doesn't,
        // since root bypasses POSIX read permissions).
        let r = TestRepo::new();
        // `parent` is a regular file; treating "parent/child" as a path
        // forces at_path to fail when it tries to enter `parent` as a dir.
        r.write("parent", b"i'm a file");
        let snap = r.snapshot();
        assert_eq!(
            snap.lookup(&r.root().join("parent/child")),
            PorcelainCode::UNTRACKED,
        );
    }

    #[test]
    fn dot_git_dir_looks_up_as_blank() {
        // The repo's own metadata isn't source-controlled. The git column
        // should render nothing, not a spurious `?` glyph.
        let r = TestRepo::new();
        assert_eq!(
            r.snapshot().lookup(&r.root().join(".git")),
            PorcelainCode::BLANK,
        );
    }

    #[test]
    fn dot_git_descendants_look_up_as_blank() {
        // The short-circuit's prefix clause catches `.git/HEAD`,
        // `.git/config`, and anything else nested under the gitdir.
        let r = TestRepo::new();
        let snap = r.snapshot();
        assert_eq!(
            snap.lookup(&r.root().join(".git/HEAD")),
            PorcelainCode::BLANK,
        );
        assert_eq!(
            snap.lookup(&r.root().join(".git/config")),
            PorcelainCode::BLANK,
        );
    }

    #[test]
    fn display_code_for_dot_git_is_blank() {
        // Renderer-facing API: must also report BLANK so a `freshl ls -a`
        // row for `.git` shows no glyph.
        let r = TestRepo::new();
        assert_eq!(
            r.snapshot().display_code_for(&r.root().join(".git"), true),
            PorcelainCode::BLANK,
        );
    }

    #[test]
    fn display_code_for_consults_exclude_stack_on_missing_path() {
        // Cold-path exercise for `display_code_for`'s `|| Some(is_directory)`
        // resolver, which is a distinct closure type from `lookup`'s
        // `symlink_metadata` resolver. A path that doesn't exist on disk
        // and isn't in the index falls through `statuses` and
        // `tracked_prefixes`; `path_is_excluded` then calls the resolver
        // to pick DIR vs FILE mode for the exclude check.
        let r = TestRepo::new();
        r.write(".gitignore", b"*.log\n")
            .write("anchor", b"x")
            .commit(&["."], "init");
        assert_eq!(
            r.snapshot()
                .display_code_for(&r.root().join("missing.log"), false),
            PorcelainCode::IGNORED,
        );
    }

    #[test]
    fn submodule_dot_git_linkfile_resolves_to_clean() {
        // The submodule's `.git` linkfile is at a nested path
        // (`submod/.git`), so the `.git` short-circuit doesn't fire
        // (`starts_with` is component-wise; the first component is
        // `submod`). It resolves to CLEAN via the ancestor walk, because
        // `submod` is in `submodule_roots`. This pins that the short-
        // circuit doesn't over-match nested `.git` basenames. Setup
        // mirrors submodule_contents_resolve_to_clean.
        let r = TestRepo::new();
        let inner = tempfile::tempdir().unwrap();
        run_git(inner.path(), &["init", "-q", "-b", "main"]);
        run_git(inner.path(), &["config", "user.email", "t@example.invalid"]);
        run_git(inner.path(), &["config", "user.name", "t"]);
        std::fs::write(inner.path().join("inside"), b"x").unwrap();
        run_git(inner.path(), &["add", "inside"]);
        run_git(inner.path(), &["commit", "-q", "-m", "inner"]);
        let inner_url = format!("file://{}", inner.path().display());
        let status = Command::new("git")
            .arg("-C")
            .arg(r.root())
            .args(["-c", "protocol.file.allow=always"])
            .args(["submodule", "add", "-q", &inner_url, "submod"])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("HOME", r.root())
            .status()
            .unwrap();
        assert!(status.success(), "git submodule add failed");
        r.commit(&["."], "add submod");
        assert_eq!(
            r.snapshot().lookup(&r.root().join("submod/.git")),
            PorcelainCode::CLEAN,
        );
    }

    #[test]
    fn dot_git_only_special_at_real_gitdir() {
        // A stray file named `.git` inside a subdirectory (not a
        // submodule, not a nested repo registered with the parent) is
        // *not* this repo's metadata. The short-circuit's component-wise
        // `starts_with` check leaves `subdir/.git` alone (its first
        // component is `subdir`, not `.git`); the ancestor walk inherits
        // UNTRACKED from `subdir`, which gix emits as collapsed-untracked.
        // This pins the check against a refactor to `rel.file_name() ==
        // Some(".git")`, which would over-match here.
        let r = TestRepo::new();
        r.write("subdir/.git", b"not a real linkfile\n");
        let snap = r.snapshot();
        assert_eq!(snap.lookup(&r.root().join(".git")), PorcelainCode::BLANK);
        assert_eq!(
            snap.lookup(&r.root().join("subdir/.git")),
            PorcelainCode::UNTRACKED,
        );
    }

    #[test]
    fn discover_returns_none_for_corrupt_index() {
        // `assemble_snapshot` propagates an `index_or_empty` failure as
        // `None` — a corrupt repo can't satisfy the lookup invariant
        // (we can't tell what's tracked) so refusing to build a snapshot
        // is the right behaviour. The corruption here is a valid header
        // signature with an unsupported version number, which gix rejects
        // cleanly instead of panicking on truncated data.
        let r = TestRepo::new();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"DIRC"); // signature
        bytes.extend_from_slice(&0x99_u32.to_be_bytes()); // unsupported version
        bytes.extend_from_slice(&0_u32.to_be_bytes()); // entry count
        // Pad with the trailing SHA-1 (20 zero bytes) so the truncation
        // check doesn't trigger before the version check.
        bytes.extend_from_slice(&[0u8; 20]);
        std::fs::write(r.root().join(".git/index"), &bytes).unwrap();
        assert!(discover(r.root()).is_none());
    }
}
