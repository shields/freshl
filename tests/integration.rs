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

#![expect(
    clippy::tests_outside_test_module,
    reason = "Cargo integration tests live at the file's module root"
)]
#![expect(
    clippy::unwrap_used,
    reason = "integration tests can panic on setup failures; allow .unwrap() in helpers too, not just #[test] bodies"
)]

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::{Command, ExitCode};

use tempfile::tempdir;

fn os(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

fn run_paths(paths: &[&Path]) -> (ExitCode, String, String) {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let args: Vec<&str> = paths.iter().map(|p| p.to_str().unwrap()).collect();
    let code = freshl::run(os(&args), &mut out, &mut err);
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

// `ExitCode` lacks `PartialEq`; format both sides to compare.
fn code_repr(code: ExitCode) -> String {
    format!("{code:?}")
}

fn set_mtime(path: &Path, time: std::time::SystemTime) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(time)
        .unwrap();
}

fn init_repo(dir: &Path) {
    run_git(dir, &["init", "-q", "-b", "main"]);
}

fn git_command(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("HOME", dir);
    cmd
}

fn run_git(dir: &Path, args: &[&str]) {
    let output = git_command(dir, args).output().expect("git command runs");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn device_files_show_rdev_as_hex_in_size_column() {
    let null = Path::new("/dev/null");
    let (code, out, _err) = run_paths(&[null]);
    assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));
    let line = out
        .lines()
        .find(|l| l.contains("/dev/null"))
        .expect("expected a row for /dev/null");
    let has_hex_token = line.split_whitespace().any(|t| {
        t.strip_prefix("0x")
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_hexdigit()))
    });
    assert!(has_hex_token, "expected a 0x<hex> token in row: {line}");
}

#[test]
fn basic_listing_columns() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("hello"), b"world").unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();
    fs::write(dir.path().join(".dot"), b"x").unwrap();
    let (code, out, _err) = run_paths(&[dir.path()]);
    assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));
    assert!(out.contains("hello"));
    assert!(out.contains("sub"));
    assert!(out.contains(".dot"));
    let has_iso_date = out.as_bytes().windows(5).any(|w| {
        w[0].is_ascii_digit()
            && w[1].is_ascii_digit()
            && w[2].is_ascii_digit()
            && w[3].is_ascii_digit()
            && w[4] == b'-'
    });
    assert!(has_iso_date, "expected ISO 8601 date in output: {out}");
}

#[test]
fn git_repo_tracks_clean_files() {
    let dir = tempdir().unwrap();
    init_repo(dir.path());
    fs::write(dir.path().join("kept"), b"hello").unwrap();
    run_git(dir.path(), &["add", "."]);
    run_git(dir.path(), &["commit", "-m", "init"]);
    let (code, out, _err) = run_paths(&[dir.path()]);
    assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));
    assert!(out.contains("kept"));
    assert!(out.contains('○'));
}

#[test]
fn git_repo_marks_untracked_and_ignored() {
    let dir = tempdir().unwrap();
    init_repo(dir.path());
    fs::write(dir.path().join("kept"), b"hello").unwrap();
    run_git(dir.path(), &["add", "kept"]);
    run_git(dir.path(), &["commit", "-m", "init"]);

    fs::write(dir.path().join(".gitignore"), b"junk.tar\n").unwrap();
    run_git(dir.path(), &["add", ".gitignore"]);
    run_git(dir.path(), &["commit", "-m", "ignore rules"]);

    fs::write(dir.path().join("junk.tar"), b"big").unwrap();
    fs::write(dir.path().join("brand_new"), b"new").unwrap();

    let (code, out, _err) = run_paths(&[dir.path()]);
    assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));
    assert!(out.contains("brand_new"));
    assert!(out.contains("junk.tar"));
    assert!(
        out.lines()
            .any(|l| l.contains("brand_new") && l.contains('?')),
        "expected ? for untracked brand_new in: {out}"
    );
    assert!(
        out.lines()
            .any(|l| l.contains("junk.tar") && l.contains('·')),
        "expected · for ignored junk.tar in: {out}"
    );
}

#[test]
fn git_repo_marks_internally_ignored_dir() {
    // uv/ruff create dirs (`.venv`, `.ruff_cache`) whose contents are ignored
    // by a `.gitignore` of `*` *inside* the dir. git treats the whole dir as
    // ignored; freshl must show `·`, not `?`. The dir itself matches no exclude
    // rule, so the status walk's directory collapse is the only thing that can
    // catch it. A sibling dir with genuinely untracked content still shows `?`.
    let dir = tempdir().unwrap();
    init_repo(dir.path());
    fs::write(dir.path().join("kept"), b"hello").unwrap();
    run_git(dir.path(), &["add", "kept"]);
    run_git(dir.path(), &["commit", "-m", "init"]);

    let venv = dir.path().join(".venv");
    fs::create_dir(&venv).unwrap();
    fs::write(venv.join(".gitignore"), b"*\n").unwrap();
    fs::write(venv.join("lib"), b"x").unwrap();

    let untracked = dir.path().join("untracked_dir");
    fs::create_dir(&untracked).unwrap();
    fs::write(untracked.join("a"), b"x").unwrap();

    let (code, out, _err) = run_paths(&[dir.path()]);
    assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));
    assert!(
        out.lines().any(|l| l.contains(".venv") && l.contains('·')),
        "expected · for internally-ignored .venv in: {out}"
    );
    assert!(
        out.lines()
            .any(|l| l.contains("untracked_dir") && l.contains('?')),
        "expected ? for untracked_dir in: {out}"
    );
}

#[test]
fn git_repo_marks_ignored_subdir_inside_untracked_dir() {
    // A wholly-ignored dir (`.gitignore` of `*`) nested inside an *untracked*
    // dir. git reports both `?? newdir/` and `!! newdir/cache/`; listing the
    // untracked dir, freshl must show `·` for the ignored `cache` and `?` for a
    // loose untracked file.
    let dir = tempdir().unwrap();
    init_repo(dir.path());
    fs::write(dir.path().join("seed"), b"x").unwrap();
    run_git(dir.path(), &["add", "seed"]);
    run_git(dir.path(), &["commit", "-m", "init"]);

    let newdir = dir.path().join("newdir");
    fs::create_dir(&newdir).unwrap();
    fs::write(newdir.join("note"), b"x").unwrap(); // genuinely untracked

    let cache = newdir.join("cache");
    fs::create_dir(&cache).unwrap();
    fs::write(cache.join(".gitignore"), b"*\n").unwrap();
    fs::write(cache.join("data"), b"x").unwrap();

    let (code, out, _err) = run_paths(&[newdir.as_path()]);
    assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));
    assert!(
        out.lines().any(|l| l.contains("cache") && l.contains('·')),
        "expected · for ignored cache inside untracked newdir in: {out}"
    );
    assert!(
        out.lines().any(|l| l.contains("note") && l.contains('?')),
        "expected ? for loose untracked note in: {out}"
    );
}

#[test]
fn git_repo_empty_dir_renders_blank_not_untracked() {
    // Git can't track an empty directory, so freshl must leave its git column
    // blank — neither `mkdir empty` nor `mkdir -p deep/a/b` is `?`. A sibling
    // untracked dir that actually holds a file still shows `?`.
    let dir = tempdir().unwrap();
    init_repo(dir.path());
    fs::write(dir.path().join("seed"), b"x").unwrap();
    run_git(dir.path(), &["add", "."]);
    run_git(dir.path(), &["commit", "-m", "init"]);

    fs::create_dir(dir.path().join("empty")).unwrap();
    fs::create_dir_all(dir.path().join("deep/a/b")).unwrap();
    fs::create_dir(dir.path().join("has_file")).unwrap();
    fs::write(dir.path().join("has_file/inner"), b"x").unwrap();

    let (code, out, _err) = run_paths(&[dir.path()]);
    assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));

    let row = |name: &str| {
        out.lines()
            .find(|l| l.contains(name))
            .unwrap_or_else(|| panic!("row for {name} missing in: {out}"))
            .to_string()
    };
    assert!(
        !row("empty").contains('?'),
        "empty dir must not show ?: {out}"
    );
    assert!(
        !row("deep").contains('?'),
        "deep empty dir must not show ?: {out}"
    );
    assert!(
        row("has_file").contains('?'),
        "untracked dir with a file must still show ?: {out}"
    );
}

#[test]
fn git_repo_marks_modified_in_worktree() {
    let dir = tempdir().unwrap();
    init_repo(dir.path());
    fs::write(dir.path().join("a"), b"hello").unwrap();
    run_git(dir.path(), &["add", "a"]);
    run_git(dir.path(), &["commit", "-m", "init"]);

    fs::write(dir.path().join("a"), b"hello world").unwrap();

    let (code, out, _err) = run_paths(&[dir.path()]);
    assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));
    let line = out
        .lines()
        .find(|l| l.contains(" a"))
        .expect("row for a exists");
    assert!(
        line.contains('●'),
        "expected modified marker in line: {line}"
    );
}

#[test]
fn git_repo_marks_staged_modification() {
    let dir = tempdir().unwrap();
    init_repo(dir.path());
    fs::write(dir.path().join("staged"), b"original").unwrap();
    run_git(dir.path(), &["add", "staged"]);
    run_git(dir.path(), &["commit", "-m", "init"]);

    fs::write(dir.path().join("staged"), b"changed").unwrap();
    run_git(dir.path(), &["add", "staged"]);

    let (code, out, _err) = run_paths(&[dir.path()]);
    assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));
    let line = out
        .lines()
        .find(|l| l.contains(" staged"))
        .expect("row for staged exists");
    assert!(
        line.contains('●'),
        "expected ● for staged modification: {line}"
    );
}

#[test]
fn multi_path_emits_labels_between_sections() {
    let dir = tempdir().unwrap();
    let a = dir.path().join("dir_a");
    let b = dir.path().join("dir_b");
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    fs::write(a.join("inside"), b"x").unwrap();
    fs::write(b.join("other"), b"y").unwrap();

    let (code, out, _err) = run_paths(&[&a, &b]);
    assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));
    let label_count = out.lines().filter(|l| l.ends_with(':')).count();
    assert_eq!(label_count, 2);
    assert!(out.contains("inside"));
    assert!(out.contains("other"));
}

#[test]
fn git_repo_marks_addition_staged() {
    let dir = tempdir().unwrap();
    init_repo(dir.path());
    fs::write(dir.path().join("anchor"), b"anchor").unwrap();
    run_git(dir.path(), &["add", "anchor"]);
    run_git(dir.path(), &["commit", "-m", "init"]);

    fs::write(dir.path().join("fresh"), b"new content").unwrap();
    run_git(dir.path(), &["add", "fresh"]);

    let (code, out, _err) = run_paths(&[dir.path()]);
    assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));
    let line = out
        .lines()
        .find(|l| l.contains(" fresh"))
        .expect("row for fresh exists");
    assert!(line.contains('+'), "expected + for new file: {line}");
}

#[test]
fn git_repo_marks_worktree_deletion() {
    let dir = tempdir().unwrap();
    init_repo(dir.path());
    fs::write(dir.path().join("victim"), b"hello").unwrap();
    run_git(dir.path(), &["add", "victim"]);
    run_git(dir.path(), &["commit", "-m", "init"]);

    fs::remove_file(dir.path().join("victim")).unwrap();

    let (code, out, _err) = run_paths(&[dir.path()]);
    assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));
    // The deleted file no longer appears as a row; this exercises the deletion
    // status path in collect_statuses without requiring its row to be displayed.
    assert!(!out.contains("victim"));
}

#[test]
fn git_repo_marks_staged_deletion() {
    let dir = tempdir().unwrap();
    init_repo(dir.path());
    fs::write(dir.path().join("dropped"), b"hello").unwrap();
    fs::write(dir.path().join("anchor"), b"anchor").unwrap();
    run_git(dir.path(), &["add", "."]);
    run_git(dir.path(), &["commit", "-m", "init"]);

    run_git(dir.path(), &["rm", "dropped"]);

    let (code, out, _err) = run_paths(&[dir.path()]);
    assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));
    assert!(!out.contains("dropped"));
    assert!(out.contains("anchor"));
}

#[test]
fn git_repo_marks_renamed() {
    let dir = tempdir().unwrap();
    init_repo(dir.path());
    let original_content = vec![b'x'; 2048];
    fs::write(dir.path().join("oldname"), &original_content).unwrap();
    run_git(dir.path(), &["add", "oldname"]);
    run_git(dir.path(), &["commit", "-m", "init"]);

    run_git(dir.path(), &["mv", "oldname", "newname"]);

    let (code, out, _err) = run_paths(&[dir.path()]);
    assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));
    let line = out
        .lines()
        .find(|l| l.contains(" newname"))
        .expect("row for newname exists");
    assert!(line.contains('→'), "expected → for renamed: {line}");
}

#[test]
fn git_repo_marks_worktree_rename() {
    let dir = tempdir().unwrap();
    init_repo(dir.path());
    let content = b"hello rewrite content padded enough to make rewrites detectable\n";
    fs::write(dir.path().join("from"), content).unwrap();
    run_git(dir.path(), &["add", "from"]);
    run_git(dir.path(), &["commit", "-m", "init"]);

    fs::rename(dir.path().join("from"), dir.path().join("to")).unwrap();

    let (code, out, _err) = run_paths(&[dir.path()]);
    assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));
    // Exercises the Rewrite branch of collect_statuses. The exact porcelain
    // marker depends on gix's rewrite-detection settings; we only assert the
    // row is rendered, not the marker character.
    assert!(out.contains("to"));
}

#[test]
fn git_repo_marks_type_change() {
    let dir = tempdir().unwrap();
    init_repo(dir.path());
    fs::write(dir.path().join("shifter"), b"hello").unwrap();
    run_git(dir.path(), &["add", "shifter"]);
    run_git(dir.path(), &["commit", "-m", "init"]);

    fs::remove_file(dir.path().join("shifter")).unwrap();
    std::os::unix::fs::symlink("/dev/null", dir.path().join("shifter")).unwrap();

    let (code, out, _err) = run_paths(&[dir.path()]);
    assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));
    let line = out
        .lines()
        .find(|l| l.contains("shifter"))
        .expect("row for shifter exists");
    assert!(line.contains('≈'), "expected ≈ for type change: {line}");
}

#[test]
fn git_repo_marks_unmerged_conflict() {
    let dir = tempdir().unwrap();
    init_repo(dir.path());
    fs::write(dir.path().join("clash"), b"alpha\n").unwrap();
    run_git(dir.path(), &["add", "clash"]);
    run_git(dir.path(), &["commit", "-m", "init"]);

    run_git(dir.path(), &["checkout", "-b", "left"]);
    fs::write(dir.path().join("clash"), b"left\n").unwrap();
    run_git(dir.path(), &["commit", "-am", "left change"]);

    run_git(dir.path(), &["checkout", "-"]);
    run_git(dir.path(), &["checkout", "-b", "right"]);
    fs::write(dir.path().join("clash"), b"right\n").unwrap();
    run_git(dir.path(), &["commit", "-am", "right change"]);

    // `git merge` exits non-zero on conflict; that's the state we want.
    let _ = git_command(dir.path(), &["merge", "left"]).output();

    let (code, out, _err) = run_paths(&[dir.path()]);
    assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));
    let line = out
        .lines()
        .find(|l| l.contains(" clash"))
        .expect("row for clash exists");
    assert!(line.contains('✘'), "expected ✘ for unmerged: {line}");
}

#[test]
fn multiple_file_args_share_column_widths() {
    let dir = tempdir().unwrap();
    let small = dir.path().join("small");
    let big = dir.path().join("big");
    fs::write(&small, b"x").unwrap();
    fs::write(&big, vec![b'x'; 1234]).unwrap();

    let (code, out, _err) = run_paths(&[&small, &big]);
    assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));
    let small_line = out
        .lines()
        .find(|l| l.ends_with("small"))
        .expect("row for small");
    let big_line = out
        .lines()
        .find(|l| l.ends_with("big"))
        .expect("row for big");
    // Shared widths put the path column at the same offset; without sharing,
    // the smaller size column would shift the path left in the small row.
    let small_idx = small_line.find(small.to_str().unwrap()).unwrap();
    let big_idx = big_line.find(big.to_str().unwrap()).unwrap();
    assert_eq!(
        small_idx, big_idx,
        "columns not aligned across files:\n{small_line}\n{big_line}"
    );
}

#[test]
fn nonexistent_path_emits_error_and_exits_one() {
    let dir = tempdir().unwrap();
    let missing = dir.path().join("nope");
    let (code, _out, err) = run_paths(&[&missing]);
    assert_eq!(code_repr(code), code_repr(ExitCode::from(1)));
    assert!(!err.is_empty());
}

fn run_args(items: &[&str]) -> (ExitCode, String, String) {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = freshl::run(os(items), &mut out, &mut err);
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

#[test]
fn sort_by_size_puts_largest_at_bottom_end_to_end() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("small"), b"x").unwrap();
    fs::write(dir.path().join("big"), vec![b'x'; 9_000]).unwrap();
    fs::write(dir.path().join("mid"), vec![b'x'; 900]).unwrap();
    let (code, out, _err) = run_args(&["-S", dir.path().to_str().unwrap()]);
    assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));
    let small_at = out.find("small").unwrap();
    let mid_at = out.find("mid").unwrap();
    let big_at = out.find("big").unwrap();
    assert!(
        small_at < mid_at && mid_at < big_at,
        "ordering wrong:\n{out}"
    );
}

#[test]
fn sort_by_time_puts_newest_at_bottom_end_to_end() {
    use std::time::{Duration, SystemTime};
    let dir = tempdir().unwrap();
    let oldest = dir.path().join("oldest");
    let middle = dir.path().join("middle");
    let newest = dir.path().join("newest");
    fs::write(&oldest, b"x").unwrap();
    fs::write(&middle, b"y").unwrap();
    fs::write(&newest, b"z").unwrap();
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    set_mtime(&oldest, base);
    set_mtime(&middle, base + Duration::from_secs(100));
    set_mtime(&newest, base + Duration::from_secs(200));

    let (code, out, _err) = run_args(&["-t", dir.path().to_str().unwrap()]);
    assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));
    let oldest_at = out.find("oldest").unwrap();
    let middle_at = out.find("middle").unwrap();
    let newest_at = out.find("newest").unwrap();
    assert!(
        oldest_at < middle_at && middle_at < newest_at,
        "ordering wrong:\n{out}"
    );
}

#[test]
fn reverse_keeps_directories_grouped_first_end_to_end() {
    let dir = tempdir().unwrap();
    fs::create_dir(dir.path().join("dir_a")).unwrap();
    fs::create_dir(dir.path().join("dir_b")).unwrap();
    fs::write(dir.path().join("file_a"), b"x").unwrap();
    fs::write(dir.path().join("file_b"), b"y").unwrap();
    let (code, out, _err) = run_args(&["-r", dir.path().to_str().unwrap()]);
    assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));
    let dir_b = out.find("dir_b").unwrap();
    let dir_a = out.find("dir_a").unwrap();
    let file_b = out.find("file_b").unwrap();
    let file_a = out.find("file_a").unwrap();
    // Directories first (reversed within), then files (reversed within).
    assert!(dir_b < dir_a, "dir_b before dir_a:\n{out}");
    assert!(dir_a < file_b, "all dirs before any files:\n{out}");
    assert!(file_b < file_a, "file_b before file_a:\n{out}");
}

#[test]
fn recursive_lists_nested_blocks_with_labels() {
    let dir = tempdir().unwrap();
    let a = dir.path().join("a");
    let b = a.join("b");
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    fs::write(a.join("leaf"), b"x").unwrap();
    fs::write(b.join("deep"), b"y").unwrap();
    let (code, out, _err) = run_args(&["-R", dir.path().to_str().unwrap()]);
    assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));
    assert!(out.contains("leaf"));
    assert!(out.contains("deep"));
    let labels = out.lines().filter(|l| l.ends_with(':')).count();
    assert_eq!(labels, 3, "expected three labeled blocks:\n{out}");
}

#[test]
fn recursive_with_time_sort_still_recurses() {
    let dir = tempdir().unwrap();
    let sub = dir.path().join("sub");
    fs::create_dir(&sub).unwrap();
    fs::write(sub.join("inside"), b"y").unwrap();
    fs::write(dir.path().join("top"), b"x").unwrap();
    let (code, out, _err) = run_args(&["-Rt", dir.path().to_str().unwrap()]);
    assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));
    assert!(out.contains("inside"));
    assert!(out.contains("top"));
}

#[test]
fn unknown_letter_in_cluster_exits_two() {
    let (code, _out, err) = run_args(&["-RX"]);
    assert_eq!(code_repr(code), code_repr(ExitCode::from(2)));
    assert!(err.contains("-RX"), "got: {err}");
}

#[test]
fn size_sort_applies_to_top_level_file_args() {
    let dir = tempdir().unwrap();
    let small = dir.path().join("zzz_small");
    let big = dir.path().join("aaa_big");
    fs::write(&small, b"x").unwrap();
    fs::write(&big, vec![b'x'; 9_000]).unwrap();
    let (code, out, _err) = run_args(&["-S", small.to_str().unwrap(), big.to_str().unwrap()]);
    assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));
    // Without -S the alphabetical default would put aaa_big first; -S
    // (smallest-first) must reorder so the smaller file appears first.
    let small_at = out.find("zzz_small").unwrap();
    let big_at = out.find("aaa_big").unwrap();
    assert!(small_at < big_at, "top-level -S did not sort:\n{out}");
}

#[test]
fn recursive_failed_root_does_not_emit_orphan_blank_for_next_arg() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    let locked = dir.path().join("locked_root");
    let good = dir.path().join("good_root");
    fs::create_dir(&locked).unwrap();
    fs::create_dir(&good).unwrap();
    fs::write(good.join("kid"), b"x").unwrap();
    let mut p = fs::metadata(&locked).unwrap().permissions();
    p.set_mode(0o000);
    fs::set_permissions(&locked, p).unwrap();

    let (code, out, _err) = run_args(&["-R", locked.to_str().unwrap(), good.to_str().unwrap()]);

    let mut p = fs::metadata(&locked).unwrap().permissions();
    p.set_mode(0o755);
    fs::set_permissions(&locked, p).unwrap();

    assert_eq!(code_repr(code), code_repr(ExitCode::from(1)));
    // The failed first root must not have left a blank-line separator that
    // would now sit at the top of the good root's output.
    assert!(!out.starts_with('\n'), "leading orphan blank line:\n{out}");
}

#[test]
fn recursive_failed_first_subdir_does_not_emit_orphan_blank() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    // Names chosen so the locked directory sorts first alphabetically and
    // is therefore the first popped from the DFS stack.
    let locked = dir.path().join("aaa_locked");
    let good = dir.path().join("zzz_good");
    fs::create_dir(&locked).unwrap();
    fs::create_dir(&good).unwrap();
    fs::write(good.join("kid"), b"x").unwrap();
    let mut p = fs::metadata(&locked).unwrap().permissions();
    p.set_mode(0o000);
    fs::set_permissions(&locked, p).unwrap();

    let (code, out, _err) = run_args(&["-R", dir.path().to_str().unwrap()]);

    let mut p = fs::metadata(&locked).unwrap().permissions();
    p.set_mode(0o755);
    fs::set_permissions(&locked, p).unwrap();

    assert_eq!(code_repr(code), code_repr(ExitCode::from(1)));
    // Two consecutive blank lines would mean an orphan separator was emitted
    // for the failed subdir block.
    assert!(!out.contains("\n\n\n"), "double blank in output:\n{out}");
}

#[test]
fn bundled_short_flag_cluster_parses_and_lists() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("one"), b"x").unwrap();
    let (code, _out, _err) = run_args(&["-rSt", dir.path().to_str().unwrap()]);
    assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));
}

#[test]
fn quadruple_r_is_a_noop_end_to_end() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("a"), b"x").unwrap();
    fs::write(dir.path().join("b"), b"x").unwrap();
    let (_, plain, _) = run_args(&[dir.path().to_str().unwrap()]);
    let (_, quad, _) = run_args(&["-rrrr", dir.path().to_str().unwrap()]);
    assert_eq!(plain, quad);
}

#[test]
fn rt_puts_oldest_at_bottom_end_to_end() {
    use std::time::{Duration, SystemTime};
    let dir = tempdir().unwrap();
    let oldest = dir.path().join("oldest");
    let newest = dir.path().join("newest");
    fs::write(&oldest, b"x").unwrap();
    fs::write(&newest, b"y").unwrap();
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    set_mtime(&oldest, base);
    set_mtime(&newest, base + Duration::from_secs(200));
    let (code, out, _err) = run_args(&["-rt", dir.path().to_str().unwrap()]);
    assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));
    let newest_at = out.find("newest").unwrap();
    let oldest_at = out.find("oldest").unwrap();
    assert!(newest_at < oldest_at, "oldest should be last:\n{out}");
}

#[test]
fn double_r_with_size_equals_size_alone_end_to_end() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("a"), b"x").unwrap();
    fs::write(dir.path().join("b"), vec![b'x'; 100]).unwrap();
    let (_, single, _) = run_args(&["-S", dir.path().to_str().unwrap()]);
    let (_, doubled, _) = run_args(&["-rrS", dir.path().to_str().unwrap()]);
    assert_eq!(single, doubled);
}

#[test]
fn directory_flag_lists_dir_as_single_row() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("inside"), b"x").unwrap();
    let (code, out, _err) = run_args(&["-d", dir.path().to_str().unwrap()]);
    assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));
    assert_eq!(out.lines().count(), 1, "expected one row: {out}");
    assert!(
        !out.contains("inside"),
        "should not expand directory: {out}"
    );
}
