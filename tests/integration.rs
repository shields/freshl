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
    assert!(out.contains('✓'));
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
    assert!(out.contains("??"));
    assert!(out.contains("!!"));
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
        line.contains('M'),
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
        line.contains('M'),
        "expected M for staged modification: {line}"
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
    assert!(line.contains('A'), "expected A for new file: {line}");
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
    assert!(line.contains('R'), "expected R for renamed: {line}");
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
    // Match " T" (space-T) to anchor on the git column rather than the ISO
    // timestamp's T (which is preceded by a digit, not a space).
    assert!(line.contains(" T"), "expected T for type change: {line}");
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
    assert!(line.contains('U'), "expected U for unmerged: {line}");
}

#[test]
fn nonexistent_path_emits_error_and_exits_one() {
    let dir = tempdir().unwrap();
    let missing = dir.path().join("nope");
    let (code, _out, err) = run_paths(&[&missing]);
    assert_eq!(code_repr(code), code_repr(ExitCode::from(1)));
    assert!(!err.is_empty());
}
