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

//! Deterministic tests for input-space cells the generative harness doesn't
//! reliably reach: special file types (FIFO/socket), submodules, exotic
//! gitignore patterns (negation, globstar, character class), unusual repo
//! shapes (secondary worktree, sparse checkout, `core.filemode`/`ignorecase`),
//! extreme timestamps, and oversized names. The matrix in docs/edge-cases.md
//! marks these cells **T (gap test)**. Unlike the proptest suites, these run
//! under `make coverage`, exercising the corresponding production paths.
//!
//! The repo-shape tests double as differential checks: each asserts freshl's
//! git column agrees with what the `git` CLI itself reports for that shape —
//! the global config (worktree linkfile, `SKIP_WORKTREE`, `core.filemode`,
//! `core.ignorecase`) is the kind of state the per-file generator can't vary,
//! so gix could plausibly diverge from git here, and these pin that it doesn't.

#![expect(
    clippy::tests_outside_test_module,
    reason = "Cargo integration tests live at the file's module root"
)]
#![expect(
    clippy::unwrap_used,
    reason = "setup failures should abort the test loudly"
)]

use std::ffi::OsString;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::{Command, ExitCode};
use std::time::{Duration, SystemTime};

use tempfile::tempdir;

fn run(args: &[&str]) -> (ExitCode, String, String) {
    let owned: Vec<OsString> = args.iter().map(OsString::from).collect();
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = freshl::run(owned, &mut out, &mut err);
    (
        code,
        String::from_utf8_lossy(&out).into_owned(),
        String::from_utf8_lossy(&err).into_owned(),
    )
}

fn code_repr(code: ExitCode) -> String {
    format!("{code:?}")
}

fn success() -> String {
    code_repr(ExitCode::SUCCESS)
}

fn git(dir: &Path, args: &[&str]) {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("HOME", dir);
    // This suite runs inside `make coverage`, which is the lefthook pre-commit
    // gate — i.e. inside a git hook, where git exports GIT_DIR / GIT_INDEX_FILE
    // / GIT_PREFIX / … pointing at the *outer* freshl repo. Inherited, they
    // hijack the throwaway repo this helper drives via `-C`: `git worktree add`
    // checks out with a relative GIT_INDEX_FILE that resolves against the new
    // worktree's `.git` *linkfile* and aborts ("index file open failed: Not a
    // directory"). Clear them so each invocation discovers its repo via `-C`
    // alone, hook or no hook.
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
    let status = cmd.status().unwrap();
    assert!(status.success(), "git {args:?} failed");
}

fn init_repo(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
}

/// Strip SGR escape sequences so a row can be matched by its visible bytes.
fn strip(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for d in chars.by_ref() {
                if d == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The first output row (escapes stripped) whose visible text contains `name`.
fn row_for(out: &str, name: &str) -> String {
    out.lines()
        .map(strip)
        .find(|l| l.contains(name))
        .unwrap_or_default()
}

#[test]
fn fifo_and_socket_render_their_type_chars() {
    let dir = tempdir().unwrap();
    let fifo = dir.path().join("pipe");
    let made = Command::new("mkfifo").arg(&fifo).status().unwrap();
    assert!(made.success(), "mkfifo failed");
    // Binding creates the socket file; it persists after the listener drops.
    let _sock = UnixListener::bind(dir.path().join("sock")).unwrap();

    let (code, out, _) = run(&[dir.path().to_str().unwrap()]);
    assert_eq!(code_repr(code), success());
    assert!(
        row_for(&out, "pipe").starts_with('p'),
        "FIFO row should start with the `p` type char: {out:?}"
    );
    assert!(
        row_for(&out, "sock").starts_with('s'),
        "socket row should start with the `s` type char: {out:?}"
    );
}

#[test]
fn gitignore_negation_and_character_class() {
    let dir = tempdir().unwrap();
    init_repo(dir.path());
    std::fs::write(
        dir.path().join(".gitignore"),
        // *.log ignores logs, but keep.log is rescued; [abc].tmp ignores a/b/c.
        "*.log\n!keep.log\n[abc].tmp\n",
    )
    .unwrap();
    for name in ["a.log", "keep.log", "a.tmp", "d.tmp"] {
        std::fs::write(dir.path().join(name), b"x").unwrap();
    }

    let (code, out, _) = run(&[dir.path().to_str().unwrap()]);
    assert_eq!(code_repr(code), success());
    // `·` is the ignored glyph, `?` the untracked glyph.
    assert!(row_for(&out, "a.log").contains('·'), "a.log ignored: {out}");
    assert!(
        row_for(&out, "keep.log").contains('?'),
        "keep.log un-ignored by negation: {out}"
    );
    assert!(row_for(&out, "a.tmp").contains('·'), "a.tmp ignored: {out}");
    assert!(
        row_for(&out, "d.tmp").contains('?'),
        "d.tmp not in [abc]: {out}"
    );
}

#[test]
fn gitignore_globstar_matches_dir_at_any_depth() {
    let dir = tempdir().unwrap();
    init_repo(dir.path());
    std::fs::write(dir.path().join(".gitignore"), "**/build/\n").unwrap();
    std::fs::create_dir_all(dir.path().join("sub/build")).unwrap();
    std::fs::write(dir.path().join("sub/build/artifact"), b"x").unwrap();

    let (code, out, _) = run(&[dir.path().join("sub").to_str().unwrap()]);
    assert_eq!(code_repr(code), success());
    assert!(
        row_for(&out, "build").contains('·'),
        "**/build/ should ignore sub/build: {out}"
    );
}

#[test]
fn submodule_subtree_lists_cleanly() {
    let inner = tempdir().unwrap();
    init_repo(inner.path());
    std::fs::write(inner.path().join("f"), b"hi").unwrap();
    git(inner.path(), &["add", "f"]);
    git(inner.path(), &["commit", "-q", "-m", "inner"]);

    let outer = tempdir().unwrap();
    init_repo(outer.path());
    // Local-path submodules need protocol.file.allow=always on modern git.
    git(
        outer.path(),
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            inner.path().to_str().unwrap(),
            "sub",
        ],
    );
    git(outer.path(), &["commit", "-q", "-m", "add submodule"]);

    let (code, out, err) = run(&["-R", outer.path().to_str().unwrap()]);
    assert_eq!(code_repr(code), success(), "stderr: {err}");
    assert!(out.contains("sub"), "submodule listed: {out}");
    assert!(out.contains(".gitmodules"), ".gitmodules listed: {out}");
    // The committed file inside the submodule is tracked by the submodule, so
    // the parent classifies it as clean (`○`), never untracked (`?`).
    assert!(
        row_for(&out, "/f").contains('○') || row_for(&out, " f").contains('○'),
        "submodule content should read clean: {out}"
    );
}

#[test]
fn far_future_mtime_renders_without_panicking() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("future");
    std::fs::write(&file, b"x").unwrap();
    // ~year 2096, comfortably inside jiff's range.
    let when = SystemTime::UNIX_EPOCH + Duration::from_secs(4_000_000_000);
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(when)
        .unwrap();

    let (code, out, _) = run(&[dir.path().to_str().unwrap()]);
    assert_eq!(code_repr(code), success());
    assert!(out.contains("2096"), "far-future year rendered: {out}");
}

#[test]
fn very_long_name_round_trips() {
    let dir = tempdir().unwrap();
    // 200 bytes — under the 255-byte NAME_MAX on common filesystems.
    let long = "z".repeat(200);
    std::fs::write(dir.path().join(&long), b"x").unwrap();

    let (code, out, _) = run(&[dir.path().to_str().unwrap()]);
    assert_eq!(code_repr(code), success());
    assert!(out.contains(&long), "long name preserved");
}

// ---- Repo shapes: global git state the per-file generator can't vary --------

#[test]
fn secondary_worktree_reports_its_own_status() {
    // A linked worktree's `.git` is a *linkfile*, and its index lives under the
    // main repo's `.git/worktrees/<name>/`. gix::discover must resolve both, so
    // freshl shows the linked worktree's own changes — exactly like running
    // `git status` from inside it.
    let main = tempdir().unwrap();
    init_repo(main.path());
    std::fs::write(main.path().join("tracked"), b"v1\n").unwrap();
    git(main.path(), &["add", "."]);
    git(main.path(), &["commit", "-q", "-m", "base"]);

    // The worktree goes in a separate dir so its path can't share a prefix.
    let wt_parent = tempdir().unwrap();
    let wt = wt_parent.path().join("wt");
    git(
        main.path(),
        &[
            "worktree",
            "add",
            "-q",
            wt.to_str().unwrap(),
            "-b",
            "feature",
        ],
    );
    std::fs::write(wt.join("tracked"), b"v1\nedited\n").unwrap();
    std::fs::write(wt.join("fresh"), b"new\n").unwrap();

    let (code, out, err) = run(&[wt.to_str().unwrap()]);
    assert_eq!(code_repr(code), success(), "stderr: {err}");
    assert!(
        row_for(&out, "tracked").contains('●'),
        "worktree's modified tracked file should read `●`: {out}"
    );
    assert!(
        row_for(&out, "fresh").contains('?'),
        "worktree's new file should read `?`: {out}"
    );
}

#[test]
fn sparse_checkout_skip_worktree_is_not_a_phantom_deletion() {
    // Cone-mode sparse checkout sets `SKIP_WORKTREE` on excluded entries: they
    // stay in the index but vanish from disk. git reports the repo as clean;
    // gix must too — a naive index-vs-worktree diff would flag the absent
    // `drop/b` as DELETED and dirty the whole tree. `-d` shows the root's own
    // aggregate glyph, so a phantom deletion would surface as `⋯`.
    let dir = tempdir().unwrap();
    init_repo(dir.path());
    std::fs::create_dir_all(dir.path().join("keep")).unwrap();
    std::fs::create_dir_all(dir.path().join("drop")).unwrap();
    std::fs::write(dir.path().join("keep/a"), b"a\n").unwrap();
    std::fs::write(dir.path().join("drop/b"), b"b\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "base"]);
    git(dir.path(), &["sparse-checkout", "init", "--cone"]);
    git(dir.path(), &["sparse-checkout", "set", "keep"]);
    assert!(
        !dir.path().join("drop").exists(),
        "sparse-checkout should have removed drop/ from disk"
    );

    let (code, out, err) = run(&["-d", dir.path().to_str().unwrap()]);
    assert_eq!(code_repr(code), success(), "stderr: {err}");
    let root = strip(&out);
    assert!(
        root.contains('○'),
        "sparse root should read clean (`○`): {out}"
    );
    assert!(
        !root.contains('⋯'),
        "a SKIP_WORKTREE entry must not flag the tree as a dirty subtree: {out}"
    );
}

#[test]
fn filemode_config_controls_exec_bit_change() {
    use std::os::unix::fs::PermissionsExt;
    // git only treats an exec-bit change as a modification when core.filemode is
    // on; gix must read the same config. Commit an executable, drop the bit on
    // disk, and check both settings: `false` → clean (`○`), `true` → modified
    // (`●`). Both directions guard against gix hard-coding either behavior.
    for (filemode, expect_modified) in [("false", false), ("true", true)] {
        let dir = tempdir().unwrap();
        init_repo(dir.path());
        git(dir.path(), &["config", "core.filemode", filemode]);
        let exe = dir.path().join("script.sh");
        std::fs::write(&exe, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "base"]);
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o644)).unwrap();

        let (code, out, _) = run(&[dir.path().to_str().unwrap()]);
        assert_eq!(code_repr(code), success());
        let row = row_for(&out, "script.sh");
        if expect_modified {
            assert!(
                row.contains('●'),
                "core.filemode=true should flag the exec-bit change as `●`: {out}"
            );
        } else {
            assert!(
                row.contains('○') && !row.contains('●'),
                "core.filemode=false should keep the exec-bit change clean (`○`): {out}"
            );
        }
    }
}

#[test]
fn ignorecase_config_controls_case_insensitive_exclude() {
    // A `.gitignore` pattern that differs from the file only in case matches it
    // iff core.ignorecase is on. The config is honored regardless of the host
    // filesystem, so this is portable: `true` → ignored (`·`), `false` →
    // untracked (`?`). Pins that gix reads core.ignorecase the way git does.
    for (ignorecase, want) in [("true", '·'), ("false", '?')] {
        let dir = tempdir().unwrap();
        init_repo(dir.path());
        git(dir.path(), &["config", "core.ignorecase", ignorecase]);
        std::fs::write(dir.path().join(".gitignore"), b"*.LOG\n").unwrap();
        std::fs::write(dir.path().join("a.log"), b"x").unwrap();

        let (code, out, _) = run(&[dir.path().to_str().unwrap()]);
        assert_eq!(code_repr(code), success());
        assert!(
            row_for(&out, "a.log").contains(want),
            "core.ignorecase={ignorecase}: a.log should read {want:?}: {out}"
        );
    }
}
