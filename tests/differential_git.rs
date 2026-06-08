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

//! Differential test: freshl's git column vs. the real `git` CLI.
//!
//! freshl reimplements `git status --porcelain --ignored` semantics on top of
//! the `gix` *library*. The `git` *binary* is an independent implementation of
//! the same spec, so diffing freshl against it over randomly generated
//! worktrees turns "did we anticipate this edge case?" into "does the oracle
//! disagree here?" — and the disagreements are the unanticipated edge cases.
//! This is the check that would have caught the empty-untracked-dir bug
//! automatically: git reports nothing for `x/y/z`, freshl rendered `?`.
//!
//! For a **file**, git is an unambiguous oracle: its porcelain code maps
//! directly to freshl's glyph. For a **directory**, git assigns no code — the
//! directory glyph is freshl's own documented refinement (clean / dirty-subtree
//! / empty-untracked-blank), which we recompute here from git's *per-file*
//! output with plain set arithmetic, deliberately simpler than freshl's gix
//! walk so the two don't share a bug.
//!
//! Scope kept exact by construction (see `materialize`): the generator does not
//! produce worktree/staged **renames or copies** (the `git` CLI and gix differ
//! on unstaged rename detection — not a freshl bug) or **merge conflicts** (set
//! up out of band; covered by git.rs unit tests). Those live in hand-written
//! tests.
//!
//! Ignored *files* nest in subdirectories like any other, so the generator does
//! exercise the "dir whose only content is ignored" case — it renders `·`,
//! matching git. The directory oracle derives that from git's per-file `!`/`?`
//! output with plain set arithmetic. The generator uses only file-name ignore
//! rules (never a `dir/` rule) and keeps empty directories in their own `c{i}`
//! prefixes (never mixed into a file-bearing dir), so `--ignored=matching`
//! reports each ignored file individually rather than collapsing a directory to
//! a single `! dir/` entry, and the oracle stays exact. The remaining ambiguous
//! neighbor — an empty subdir nested *inside* a dir whose files are all ignored,
//! where git itself declines to collapse — is intentionally unspecified (see
//! docs/edge-cases.md) and never generated.

// Heavy (spawns git per case): excluded from the coverage *build* so the
// pre-commit `make coverage` hook stays fast. It still runs under `make test`
// and in CI's test job, where it does its real work.
#![cfg(not(coverage_nightly))]
#![expect(
    clippy::tests_outside_test_module,
    reason = "Cargo integration tests live at the file's module root"
)]
#![expect(
    clippy::unwrap_used,
    reason = "a generator/oracle setup failure should abort the test loudly"
)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Generated repository shape
// ---------------------------------------------------------------------------

/// One file's git state. Each is reached by a fixed recipe in `materialize`,
/// and maps to exactly one expected glyph (via git's porcelain output).
#[derive(Clone, Copy, Debug)]
enum FileState {
    Committed,        // committed, untouched            -> ○
    Untracked,        // present, never added            -> ?
    Ignored,          // matched by .gitignore (root)    -> ·
    ModifiedWorktree, // committed, then edited on disk  -> ●
    StagedModify,     // committed, edited, re-added     -> ● (staged)
    StagedAdd,        // new file, added not committed   -> +
    DeletedWorktree,  // committed, then removed on disk -> ▽
    StagedDelete,     // committed, then `git rm`        -> ▽ (staged)
    TypeChange,       // committed file -> symlink        -> ≈
}

fn state_strategy() -> impl Strategy<Value = FileState> {
    prop_oneof![
        Just(FileState::Committed),
        Just(FileState::Untracked),
        Just(FileState::Ignored),
        Just(FileState::ModifiedWorktree),
        Just(FileState::StagedModify),
        Just(FileState::StagedAdd),
        Just(FileState::DeletedWorktree),
        Just(FileState::StagedDelete),
        Just(FileState::TypeChange),
    ]
}

/// A file placed at depth 0..=2 under directories drawn from a tiny pool
/// (`d0`/`d1`/`d2`), so files share parents and form nesting. The leaf name is
/// assigned from the file's index at materialization time, so paths are unique
/// regardless of what the strategy picks.
#[derive(Clone, Debug)]
struct FileSpec {
    dirs: Vec<u8>,
    state: FileState,
}

fn file_spec_strategy() -> impl Strategy<Value = FileSpec> {
    (prop::collection::vec(0u8..3, 0..=2), state_strategy())
        .prop_map(|(dirs, state)| FileSpec { dirs, state })
}

#[derive(Clone, Debug)]
struct RepoSpec {
    files: Vec<FileSpec>,
    /// Each value is the depth of an empty directory chain (`mkdir -p`), under
    /// its own `c{i}` prefix so it never receives a file and stays genuinely
    /// empty — exercising the empty-untracked-dir → blank rule.
    empty_chains: Vec<u8>,
}

fn repo_spec_strategy() -> impl Strategy<Value = RepoSpec> {
    (
        prop::collection::vec(file_spec_strategy(), 0..=10),
        prop::collection::vec(1u8..=3, 0..=3),
    )
        .prop_map(|(files, empty_chains)| RepoSpec {
            files,
            empty_chains,
        })
}

/// The on-disk relative path for a file: its directory chain (drawn from the
/// shared `d0`/`d1`/`d2` pool) plus a unique leaf. Ignored files nest like any
/// other — a subdirectory whose only content is ignored files is exactly the
/// directory-aggregation case the oracle must get right (it renders `·`).
fn file_rel(spec: &FileSpec, idx: usize) -> PathBuf {
    let mut p = PathBuf::new();
    for d in &spec.dirs {
        p.push(format!("d{d}"));
    }
    p.push(format!("f{idx}"));
    p
}

// ---------------------------------------------------------------------------
// git plumbing
// ---------------------------------------------------------------------------

fn git_base(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("HOME", dir);
    // `make coverage` (the lefthook pre-commit gate) runs this suite *inside a
    // git hook*, where git exports GIT_DIR / GIT_INDEX_FILE / GIT_PREFIX / …
    // pointing at the *outer* freshl repo. Inherited, they hijack the throwaway
    // repo driven via `-C` — even `git init` then writes to the outer `.git`.
    // Clear them so each invocation discovers its repo via `-C` alone.
    for var in [
        "GIT_DIR",
        "GIT_INDEX_FILE",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_PREFIX",
        "GIT_OBJECT_DIRECTORY",
        "GIT_NAMESPACE",
        "GIT_CEILING_DIRECTORIES",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        cmd.env_remove(var);
    }
    cmd
}

/// Run a mutating git command; panic on failure (setup must succeed).
fn git(dir: &Path, args: &[&str]) {
    let out = git_base(dir).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Run a git command and return its raw stdout bytes (paths survive verbatim).
fn git_capture(dir: &Path, args: &[&str]) -> Vec<u8> {
    let out = git_base(dir).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

/// Build the worktree described by `spec` and return its tempdir.
fn materialize(spec: &RepoSpec) -> TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q", "-b", "main"]);

    let rels: Vec<PathBuf> = spec
        .files
        .iter()
        .enumerate()
        .map(|(i, f)| file_rel(f, i))
        .collect();

    // A committed anchor guarantees a non-empty base commit and a tracked root.
    std::fs::write(root.join(".keep"), b"keep\n").unwrap();

    // .gitignore lists each ignored file's full repo-relative path (e.g.
    // `d0/f3`). A slash-bearing pattern is anchored to the repo root, so it
    // matches exactly that one unique file — a file-name rule, never a `dir/`
    // rule, so git reports the ignored file individually rather than collapsing
    // its directory.
    let ignored: Vec<String> = spec
        .files
        .iter()
        .zip(&rels)
        .filter(|(f, _)| matches!(f.state, FileState::Ignored))
        .map(|(_, rel)| rel.to_string_lossy().into_owned())
        .collect();
    if !ignored.is_empty() {
        std::fs::write(root.join(".gitignore"), format!("{}\n", ignored.join("\n"))).unwrap();
    }

    // Phase A: write every file that needs a committed base, then commit.
    for (f, rel) in spec.files.iter().zip(&rels) {
        if needs_base(f.state) {
            write_file(root, rel, &format!("BASE-CONTENT-for-{}\n", rel.display()));
        }
    }
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "base"]);

    // Phase B: mutate committed files into their target states.
    for (f, rel) in spec.files.iter().zip(&rels) {
        let abs = root.join(rel);
        let rel_str = rel.to_string_lossy();
        match f.state {
            FileState::ModifiedWorktree => append(&abs, b"edited\n"),
            FileState::StagedModify => {
                append(&abs, b"edited\n");
                git(root, &["add", "--", &rel_str]);
            }
            FileState::DeletedWorktree => std::fs::remove_file(&abs).unwrap(),
            FileState::StagedDelete => git(root, &["rm", "-q", "--", &rel_str]),
            FileState::TypeChange => {
                std::fs::remove_file(&abs).unwrap();
                std::os::unix::fs::symlink("type-change-target", &abs).unwrap();
            }
            _ => {}
        }
    }

    // Phase C: create the files that must not be in the base commit.
    for (f, rel) in spec.files.iter().zip(&rels) {
        let rel_str = rel.to_string_lossy();
        match f.state {
            FileState::Untracked => {
                write_file(root, rel, &format!("UNTRACKED-CONTENT-for-{rel_str}\n"));
            }
            FileState::StagedAdd => {
                write_file(root, rel, &format!("STAGED-ADD-CONTENT-for-{rel_str}\n"));
                git(root, &["add", "--", &rel_str]);
            }
            FileState::Ignored => {
                write_file(root, rel, &format!("IGNORED-CONTENT-for-{rel_str}\n"));
            }
            _ => {}
        }
    }

    // Phase D: empty directory chains.
    for (i, &depth) in spec.empty_chains.iter().enumerate() {
        let mut p = root.to_path_buf();
        for _ in 0..depth {
            p.push(format!("c{i}"));
        }
        std::fs::create_dir_all(&p).unwrap();
    }

    tmp
}

const fn needs_base(state: FileState) -> bool {
    matches!(
        state,
        FileState::Committed
            | FileState::ModifiedWorktree
            | FileState::StagedModify
            | FileState::DeletedWorktree
            | FileState::StagedDelete
            | FileState::TypeChange
    )
}

fn write_file(root: &Path, rel: &Path, content: &str) {
    let abs = root.join(rel);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(abs, content).unwrap();
}

fn append(abs: &Path, bytes: &[u8]) {
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new().append(true).open(abs).unwrap();
    f.write_all(bytes).unwrap();
}

// ---------------------------------------------------------------------------
// Oracle: derive expected glyphs from the real git CLI
// ---------------------------------------------------------------------------

/// `git status --porcelain=v2` X/Y → freshl glyph. Worktree (Y) wins, index (X)
/// is the fallback — exactly `PorcelainCode::glyph`'s rule.
fn xy_glyph(x: char, y: char) -> char {
    let worktree = match y {
        'M' => Some('●'),
        'D' => Some('▽'),
        'T' => Some('≈'),
        'R' => Some('→'),
        'C' => Some('⇉'),
        _ => None,
    };
    let index = match x {
        'M' => Some('●'),
        'A' => Some('+'),
        'D' => Some('▽'),
        'R' => Some('→'),
        'C' => Some('⇉'),
        _ => None,
    };
    worktree.or(index).unwrap_or('?')
}

/// The path is the final whitespace-separated field of an ordinary/rename/
/// unmerged record (the generator never produces spaces in names).
fn last_field(text: &str) -> PathBuf {
    PathBuf::from(text.rsplit(' ').next().unwrap_or_default())
}

/// Parse `git status --porcelain=v2 -z` into a `path -> glyph` map.
fn parse_status(raw: &[u8]) -> HashMap<PathBuf, char> {
    let mut map = HashMap::new();
    // -z records are NUL-terminated; a rename/copy ("2") record is followed by
    // a separate NUL-terminated original-path record we skip.
    let records: Vec<&[u8]> = raw.split(|&b| b == 0).filter(|r| !r.is_empty()).collect();
    let mut i = 0;
    while let Some(rec) = records.get(i) {
        i += 1;
        let text = String::from_utf8_lossy(rec);
        if let Some(path) = text.strip_prefix("? ") {
            map.insert(PathBuf::from(path), '?');
        } else if let Some(path) = text.strip_prefix("! ") {
            map.insert(PathBuf::from(path), '·');
        } else if let Some(rest) = text.strip_prefix("1 ") {
            let mut xy = rest.chars();
            let g = xy_glyph(xy.next().unwrap_or(' '), xy.next().unwrap_or(' '));
            map.insert(last_field(&text), g);
        } else if let Some(rest) = text.strip_prefix("2 ") {
            let mut xy = rest.chars();
            let g = xy_glyph(xy.next().unwrap_or(' '), xy.next().unwrap_or(' '));
            map.insert(last_field(&text), g);
            i += 1; // skip the original-path record
        } else if text.starts_with("u ") {
            map.insert(last_field(&text), '✘');
        }
    }
    map
}

fn parse_tracked(raw: &[u8]) -> HashSet<PathBuf> {
    raw.split(|&b| b == 0)
        .filter(|r| !r.is_empty())
        .map(|r| PathBuf::from(String::from_utf8_lossy(r).into_owned()))
        .collect()
}

/// Expected glyph for a file path. git is authoritative: a path it reports
/// takes that glyph; otherwise a tracked path is clean (`○`).
fn expected_file_glyph(
    rel: &Path,
    status: &HashMap<PathBuf, char>,
    tracked: &HashSet<PathBuf>,
) -> char {
    if let Some(&g) = status.get(rel) {
        g
    } else if tracked.contains(rel) {
        '○'
    } else {
        // Shouldn't happen under -uall --ignored=matching; fail toward `?`.
        '?'
    }
}

/// Expected glyph for a directory, recomputed from git's per-file output with
/// plain set arithmetic — deliberately simpler than freshl's gix walk so the two
/// can't share a bug. Under `--ignored=matching --untracked-files=all` every
/// path beneath `rel` is, per git, tracked / changed (`?` counts) / ignored
/// (`·`), reported individually (the generator emits no `dir/` rule that would
/// collapse a directory). The rule:
///   - a tracked prefix (≥1 tracked descendant, or the root) is clean unless a
///     descendant is dirty — `○` / `⋯`; an ignored descendant never dirties it;
///   - otherwise an untracked descendant wins → `?`;
///   - else an ignored descendant → `·` (the "dir whose only content is
///     ignored" case);
///   - else no content at all → blank (empty untracked directory).
fn expected_dir_glyph(
    rel: &Path,
    tracked: &HashSet<PathBuf>,
    status: &HashMap<PathBuf, char>,
) -> char {
    // The repo root is always a tracked prefix in freshl (empty path seeded),
    // so it never falls into the untracked-directory branch.
    let is_tracked = rel.as_os_str().is_empty() || tracked.iter().any(|t| t.starts_with(rel));
    let under = |want: char| status.iter().any(|(p, &g)| g == want && p.starts_with(rel));
    if is_tracked {
        // Any non-ignored status under the dir makes it a dirty subtree.
        let has_dirty = status.iter().any(|(p, &g)| g != '·' && p.starts_with(rel));
        if has_dirty { '⋯' } else { '○' }
    } else if under('?') {
        '?'
    } else if under('·') {
        '·'
    } else {
        ' ' // empty untracked directory renders blank
    }
}

/// Walk the worktree (excluding `.git`), classifying every entry as a real
/// directory or a non-directory (symlinks counted as non-dir, not followed).
fn walk(root: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut dirs = vec![PathBuf::new()]; // the root itself
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(&current).unwrap().flatten() {
            let path = entry.path();
            let rel = path.strip_prefix(root).unwrap().to_path_buf();
            if rel.as_os_str() == ".git" {
                continue;
            }
            match entry.file_type() {
                Ok(ft) if ft.is_dir() => {
                    dirs.push(rel);
                    stack.push(path);
                }
                _ => files.push(rel),
            }
        }
    }
    (dirs, files)
}

// ---------------------------------------------------------------------------
// The property
// ---------------------------------------------------------------------------

fn check_one(spec: &RepoSpec) -> Result<(), TestCaseError> {
    let tmp = materialize(spec);
    let root = tmp.path();

    let status = parse_status(&git_capture(
        root,
        &[
            "status",
            "--porcelain=v2",
            "-z",
            "--ignored=matching",
            "--untracked-files=all",
        ],
    ));
    let tracked = parse_tracked(&git_capture(root, &["ls-files", "-z"]));

    let snap = freshl::git::discover(root).expect("repo discovered");
    let canon = snap.root.clone();
    let (dirs, files) = walk(&canon);

    for rel in &files {
        let actual = snap
            .display_code_for(&canon.join(rel), /* is_dir */ false)
            .glyph();
        let expected = expected_file_glyph(rel, &status, &tracked);
        prop_assert_eq!(
            actual,
            expected,
            "file {:?}: freshl={:?} git={:?} (porcelain {:?})",
            rel,
            actual,
            expected,
            status.get(rel)
        );
    }
    for rel in &dirs {
        let actual = snap
            .display_code_for(&canon.join(rel), /* is_dir */ true)
            .glyph();
        let expected = expected_dir_glyph(rel, &tracked, &status);
        prop_assert_eq!(
            actual,
            expected,
            "dir {:?}: freshl={:?} expected={:?}",
            rel,
            actual,
            expected
        );
    }
    Ok(())
}

#[test]
fn freshl_git_column_matches_git_cli() {
    // Deterministic RNG + no on-disk failure persistence: a fixed, reproducible
    // sweep over the larger-config long tail, complementing the exhaustive
    // small-repo pass below. CI can widen it via PROPTEST_CASES.
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(64);
    let mut config = Config::with_cases(cases);
    config.failure_persistence = None;
    let mut runner =
        TestRunner::new_with_rng(config, TestRng::deterministic_rng(RngAlgorithm::ChaCha));
    runner
        .run(&repo_spec_strategy(), |spec| check_one(&spec))
        .expect("freshl's git column should match the git CLI on every generated worktree");
}

const ALL_STATES: [FileState; 9] = [
    FileState::Committed,
    FileState::Untracked,
    FileState::Ignored,
    FileState::ModifiedWorktree,
    FileState::StagedModify,
    FileState::StagedAdd,
    FileState::DeletedWorktree,
    FileState::StagedDelete,
    FileState::TypeChange,
];

/// A bounded-exhaustive set of the *small* configurations where defects
/// surface — every known bug manifested in ≤2 entries, so the comparator's
/// behavior here is enumerable rather than merely sampleable. Covers: every
/// empty-dir series; every single file at the root or one level down in each
/// state; and every unordered pair of states sharing a parent directory (the
/// directory-aggregation truth table — this is where the deleted-tracked-file
/// bug lived). Because `Ignored` is one of the states and nests in `d0` like
/// the rest, the pairs include a subdirectory whose only content is ignored
/// (→ `·`) and every ignored-plus-other mix (→ `?` / `○` / `⋯`).
fn small_repo_specs() -> Vec<RepoSpec> {
    let mut specs = Vec::new();
    for chains in [Vec::new(), vec![1u8], vec![2u8]] {
        specs.push(RepoSpec {
            files: Vec::new(),
            empty_chains: chains,
        });
    }
    for state in ALL_STATES {
        for dirs in [Vec::new(), vec![0u8]] {
            for chains in [Vec::new(), vec![1u8]] {
                specs.push(RepoSpec {
                    files: vec![FileSpec {
                        dirs: dirs.clone(),
                        state,
                    }],
                    empty_chains: chains,
                });
            }
        }
    }
    for (i, &s1) in ALL_STATES.iter().enumerate() {
        for &s2 in ALL_STATES.iter().skip(i) {
            specs.push(RepoSpec {
                files: vec![
                    FileSpec {
                        dirs: vec![0u8],
                        state: s1,
                    },
                    FileSpec {
                        dirs: vec![0u8],
                        state: s2,
                    },
                ],
                empty_chains: Vec::new(),
            });
        }
    }
    specs
}

#[test]
fn small_repos_match_git_cli_exhaustively() {
    // Deterministic certainty for the small configurations; the random sweep
    // above covers the larger-config long tail.
    for spec in small_repo_specs() {
        let result = check_one(&spec);
        assert!(
            result.is_ok(),
            "exhaustive small-repo differential failed for {spec:?}: {result:?}"
        );
    }
}
