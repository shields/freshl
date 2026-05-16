#[cfg(not(unix))]
compile_error!("freshl targets POSIX file metadata and only builds on Unix.");

use std::ffi::OsString;
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub mod args;
pub mod case;
pub mod collect;
pub mod entry;
pub mod error;
pub mod format;
pub mod git;
pub mod owner;
pub mod sort;

use args::{Action, parse};
use case::{DetectorCache, ProbeDetector};
use entry::{Entry, EntryKind};
use error::Error;
use format::{Row, build_row, compute_widths, render_row};
use git::{PorcelainCode, Snapshot, SnapshotCache};
use owner::{OwnerCache, SystemDirectory};

struct Caches {
    owners: OwnerCache<SystemDirectory>,
    sensitivity: DetectorCache<ProbeDetector>,
    snapshots: SnapshotCache,
}

impl Caches {
    fn new() -> Self {
        Self {
            owners: OwnerCache::new(SystemDirectory),
            sensitivity: DetectorCache::new(ProbeDetector),
            snapshots: SnapshotCache::new(),
        }
    }
}

#[must_use]
pub fn run<I>(raw: I, stdout: &mut dyn Write, stderr: &mut dyn Write) -> ExitCode
where
    I: IntoIterator<Item = OsString>,
{
    match dispatch(raw, stdout, stderr) {
        Ok(code) => code,
        Err(err) => {
            let _ = writeln!(stderr, "{err}");
            err.exit_code()
        }
    }
}

fn dispatch<I>(
    raw: I,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<ExitCode, Error>
where
    I: IntoIterator<Item = OsString>,
{
    let action = parse(raw).map_err(|e| Error::Usage(e.message))?;
    match action {
        Action::Help => write_stdout(stdout, args::HELP.as_bytes()).map(|()| ExitCode::SUCCESS),
        Action::Version => {
            write_stdout(stdout, format!("{}\n", args::version_line()).as_bytes())
                .map(|()| ExitCode::SUCCESS)
        }
        Action::List(paths) => list(stdout, stderr, &paths),
    }
}

fn write_stdout(stdout: &mut dyn Write, bytes: &[u8]) -> Result<(), Error> {
    stdout.write_all(bytes).map_err(stdout_io)
}

fn list(
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    paths: &[PathBuf],
) -> Result<ExitCode, Error> {
    let fallback = [PathBuf::from(".")];
    let targets: &[PathBuf] = if paths.is_empty() { &fallback } else { paths };
    let multi = targets.len() > 1;
    let mut had_error = false;
    let mut caches = Caches::new();

    // Split into a batch of files and a list of directories. Files render
    // together so column widths span all file arguments (matching `ls -l
    // file1 file2 …`); each directory then renders as its own block with
    // its own widths.
    let mut files: Vec<Entry> = Vec::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for target in targets {
        match collect::entry_for_path(target) {
            Ok(entry) => {
                if entry.kind == EntryKind::Directory {
                    dirs.push(target.clone());
                } else {
                    // Display the user-supplied path so a `freshl /etc/passwd`
                    // row reads `… /etc/passwd`, not just `passwd`.
                    let mut e = entry;
                    e.name = target.as_os_str().to_os_string();
                    files.push(e);
                }
            }
            Err(source) => {
                let _ = writeln!(
                    stderr,
                    "{}",
                    Error::Io {
                        path: target.clone(),
                        source,
                    }
                );
                had_error = true;
            }
        }
    }

    let mut have_output = !files.is_empty();
    if have_output {
        render_files(stdout, &files, &mut caches)?;
    }
    for target in &dirs {
        if have_output {
            writeln!(stdout).map_err(stdout_io)?;
        }
        match list_directory(stdout, stderr, target, multi, &mut caches) {
            Ok(target_had_error) => {
                had_error |= target_had_error;
                have_output = true;
            }
            Err(e @ Error::StdoutIo(_)) => return Err(e),
            Err(e) => {
                let _ = writeln!(stderr, "{e}");
                had_error = true;
            }
        }
    }
    Ok(if had_error {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

fn list_directory(
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    target: &Path,
    show_label: bool,
    caches: &mut Caches,
) -> Result<bool, Error> {
    let listing = collect::collect_directory(target).map_err(|source| Error::Io {
        path: target.to_path_buf(),
        source,
    })?;
    if show_label {
        write_path_with_suffix(stdout, target, b":\n")?;
    }
    let mut entries = listing.entries;
    let sense = {
        let names: Vec<&std::ffi::OsStr> = entries.iter().map(|e| e.name.as_os_str()).collect();
        caches.sensitivity.sensitivity(target, &names)
    };
    sort::sort(&mut entries, sense);
    let snapshot = caches.snapshots.for_target(target);
    render_entries(stdout, &entries, &mut caches.owners, snapshot)?;
    for (path, source) in &listing.errors {
        let _ = writeln!(
            stderr,
            "{}",
            Error::Io {
                path: path.clone(),
                source: io::Error::new(source.kind(), source.to_string()),
            }
        );
    }
    Ok(!listing.errors.is_empty())
}

fn render_entries(
    stdout: &mut dyn Write,
    entries: &[Entry],
    owners: &mut OwnerCache<SystemDirectory>,
    snapshot: Option<&Snapshot>,
) -> Result<(), Error> {
    let mut rows: Vec<Row> = entries.iter().map(|e| build_row(e, owners)).collect();
    for (row, entry) in rows.iter_mut().zip(entries.iter()) {
        enrich_row(row, entry, snapshot);
    }
    let git_width = if snapshot.is_some() {
        format::git_col::WIDTH
    } else {
        0
    };
    write_rows(stdout, &rows, git_width)
}

fn render_files(
    stdout: &mut dyn Write,
    entries: &[Entry],
    caches: &mut Caches,
) -> Result<(), Error> {
    // Each file argument may live in a different repository (or none); look
    // up its snapshot one entry at a time so we don't hold multiple cache
    // borrows at once, then render everything with shared widths.
    let mut rows: Vec<Row> = Vec::with_capacity(entries.len());
    let mut any_git = false;
    for entry in entries {
        let mut row = build_row(entry, &mut caches.owners);
        let snap = caches.snapshots.for_target(&entry.path);
        if snap.is_some() {
            any_git = true;
        }
        enrich_row(&mut row, entry, snap);
        rows.push(row);
    }
    let git_width = if any_git { format::git_col::WIDTH } else { 0 };
    write_rows(stdout, &rows, git_width)
}

fn enrich_row(row: &mut Row, entry: &Entry, snapshot: Option<&Snapshot>) {
    // `Snapshot::lookup` can canonicalize the path; do it once and derive
    // both the git column and the ignored flag from the same result.
    let code = snapshot.map(|s| s.lookup(&entry.path));
    if let Some(c) = code {
        row.git = Some(format::git_col::render(c));
    }
    let ignored = code == Some(PorcelainCode::IGNORED);
    let missing = entry.kind == EntryKind::Symlink && target_is_missing(entry);
    if ignored || missing {
        row.name = format::name::format_name(entry, ignored, missing);
    }
}

fn write_rows(stdout: &mut dyn Write, rows: &[Row], git_width: usize) -> Result<(), Error> {
    let widths = compute_widths(rows);
    for row in rows {
        let line = render_row(row, widths, git_width);
        stdout.write_all(&line).map_err(stdout_io)?;
        stdout.write_all(b"\n").map_err(stdout_io)?;
    }
    Ok(())
}

fn target_is_missing(entry: &Entry) -> bool {
    let Some(target) = &entry.symlink_target else {
        return false;
    };
    let absolute = if target.is_absolute() {
        target.clone()
    } else {
        entry
            .path
            .parent()
            .map_or_else(|| target.clone(), |parent| parent.join(target))
    };
    std::fs::metadata(absolute).is_err()
}

// Write the path's raw OS bytes followed by `suffix`. Filenames on Unix are
// arbitrary byte sequences; using `Display` (which goes through
// `to_string_lossy`) would replace invalid UTF-8 with U+FFFD and break
// pipelines. TTY-aware quoting/escaping of control characters in names is a
// separate concern from byte-fidelity and is not part of this chunk.
fn write_path_with_suffix(
    stdout: &mut dyn Write,
    path: &Path,
    suffix: &[u8],
) -> Result<(), Error> {
    stdout
        .write_all(path.as_os_str().as_bytes())
        .map_err(stdout_io)?;
    stdout.write_all(suffix).map_err(stdout_io)
}

const fn stdout_io(source: std::io::Error) -> Error {
    Error::StdoutIo(source)
}

#[cfg(test)]
mod tests {
    use super::run;
    use std::ffi::OsString;
    use std::fs;
    use std::io::{self, Write};
    use tempfile::tempdir;

    fn os(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    fn code_repr(code: std::process::ExitCode) -> String {
        format!("{code:?}")
    }

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("nope"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct FailOnNewline {
        seen: usize,
        fail_after: usize,
    }

    impl FailOnNewline {
        const fn new(fail_after: usize) -> Self {
            Self {
                seen: 0,
                fail_after,
            }
        }
    }

    impl Write for FailOnNewline {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            for b in buf {
                if *b == b'\n' {
                    self.seen += 1;
                    if self.seen > self.fail_after {
                        return Err(io::Error::other("nope"));
                    }
                }
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn failing_writer_flush_is_a_noop() {
        assert!(FailingWriter.flush().is_ok());
    }

    #[test]
    fn fail_on_newline_writer_eventually_errors() {
        let mut w = FailOnNewline::new(1);
        assert!(w.write(b"first\n").is_ok());
        assert!(w.write(b"second\n").is_err());
        assert!(w.flush().is_ok());
    }

    #[test]
    fn help_writes_to_stdout_and_returns_success() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(os(&["--help"]), &mut out, &mut err);
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Usage: freshl"));
        assert!(err.is_empty());
    }

    #[test]
    fn version_writes_to_stdout() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(os(&["--version"]), &mut out, &mut err);
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("freshl "));
    }

    #[test]
    fn unknown_flag_writes_to_stderr_and_returns_two() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(os(&["--bogus"]), &mut out, &mut err);
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::from(2)));
        assert!(out.is_empty());
    }

    #[test]
    fn listing_no_args_lists_current_directory() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(os(&[]), &mut out, &mut err);
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
    }

    #[test]
    fn listing_directory_arg_prints_rows() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("file"), b"hi").unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(os(&[dir.path().to_str().unwrap()]), &mut out, &mut err);
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("file"));
        assert!(text.starts_with(" 6") || text.contains("\n 6"));
    }

    #[test]
    fn listing_file_arg_prints_one_row_with_full_path() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("only");
        fs::write(&file, b"hi").unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(os(&[file.to_str().unwrap()]), &mut out, &mut err);
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(text.contains(file.to_str().unwrap()));
    }

    #[test]
    fn listing_multiple_paths_emits_labels_and_separator() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        fs::write(a.join("inside"), b"x").unwrap();
        fs::write(b.join("other"), b"y").unwrap();

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(
            os(&[a.to_str().unwrap(), b.to_str().unwrap()]),
            &mut out,
            &mut err,
        );
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains(':'));
        assert!(text.contains("inside"));
        assert!(text.contains("other"));
    }

    #[test]
    fn listing_nonexistent_path_returns_one() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("ghost");
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(os(&[missing.to_str().unwrap()]), &mut out, &mut err);
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::from(1)));
        let stderr_text = String::from_utf8(err).unwrap();
        assert!(stderr_text.contains("ghost"));
    }

    #[test]
    fn listing_unreadable_directory_returns_one() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let locked = dir.path().join("locked");
        fs::create_dir(&locked).unwrap();
        let mut perms = fs::metadata(&locked).unwrap().permissions();
        perms.set_mode(0o000);
        fs::set_permissions(&locked, perms).unwrap();

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(os(&[locked.to_str().unwrap()]), &mut out, &mut err);

        let mut perms = fs::metadata(&locked).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&locked, perms).unwrap();

        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::from(1)));
    }

    #[test]
    fn listing_reports_per_child_stat_failures_and_returns_one() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let inner = dir.path().join("inner");
        fs::create_dir(&inner).unwrap();
        fs::write(inner.join("a"), b"hi").unwrap();
        let mut p = fs::metadata(&inner).unwrap().permissions();
        p.set_mode(0o400);
        fs::set_permissions(&inner, p).unwrap();

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(os(&[inner.to_str().unwrap()]), &mut out, &mut err);

        let mut p = fs::metadata(&inner).unwrap().permissions();
        p.set_mode(0o755);
        fs::set_permissions(&inner, p).unwrap();

        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::from(1)));
        let stderr_text = String::from_utf8(err).unwrap();
        assert!(stderr_text.contains('a'));
    }

    #[test]
    fn broken_symlink_renders_with_red_target_indicator() {
        let dir = tempdir().unwrap();
        let link = dir.path().join("dangling");
        std::os::unix::fs::symlink(dir.path().join("nope"), &link).unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(os(&[dir.path().to_str().unwrap()]), &mut out, &mut err);
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
        // The target painted red embeds the AnsiColor::Red SGR sequence.
        assert!(out.windows(2).any(|w| w == b"31"));
    }

    #[test]
    fn relative_broken_symlink_resolves_relative_to_parent() {
        let dir = tempdir().unwrap();
        let link = dir.path().join("rellink");
        std::os::unix::fs::symlink("does-not-exist", &link).unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(os(&[dir.path().to_str().unwrap()]), &mut out, &mut err);
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("rellink"));
        assert!(text.contains("does-not-exist"));
    }

    #[test]
    fn target_is_missing_handles_absolute() {
        use super::target_is_missing;
        use crate::entry::{Entry, EntryKind};
        use std::ffi::OsString;
        use std::path::PathBuf;
        use std::time::SystemTime;
        let e = Entry {
            name: OsString::from("link"),
            path: PathBuf::from("/tmp/link"),
            kind: EntryKind::Symlink,
            mode: 0,
            nlink: 0,
            uid: 0,
            gid: 0,
            size: 0,
            rdev: 0,
            mtime: SystemTime::UNIX_EPOCH,
            symlink_target: Some(PathBuf::from("/definitely/does/not/exist/anywhere")),
        };
        assert!(target_is_missing(&e));
    }

    #[test]
    fn target_is_missing_is_false_when_target_is_none() {
        use super::target_is_missing;
        use crate::entry::{Entry, EntryKind};
        use std::ffi::OsString;
        use std::path::PathBuf;
        use std::time::SystemTime;
        let e = Entry {
            name: OsString::from("file"),
            path: PathBuf::from("file"),
            kind: EntryKind::RegularFile,
            mode: 0,
            nlink: 0,
            uid: 0,
            gid: 0,
            size: 0,
            rdev: 0,
            mtime: SystemTime::UNIX_EPOCH,
            symlink_target: None,
        };
        assert!(!target_is_missing(&e));
    }

    #[test]
    fn target_is_missing_resolves_relative_when_link_path_has_no_parent() {
        use super::target_is_missing;
        use crate::entry::{Entry, EntryKind};
        use std::ffi::OsString;
        use std::path::PathBuf;
        use std::time::SystemTime;
        // `Path::new("/").parent()` returns `None`, exercising the
        // `map_or_else` no-parent arm in `target_is_missing`.
        let e = Entry {
            name: OsString::from("/"),
            path: PathBuf::from("/"),
            kind: EntryKind::Symlink,
            mode: 0,
            nlink: 0,
            uid: 0,
            gid: 0,
            size: 0,
            rdev: 0,
            mtime: SystemTime::UNIX_EPOCH,
            symlink_target: Some(PathBuf::from("does-not-exist-anywhere")),
        };
        assert!(target_is_missing(&e));
    }

    #[test]
    fn listing_continues_past_missing_paths() {
        let dir = tempdir().unwrap();
        let good = dir.path().join("good");
        fs::create_dir(&good).unwrap();
        fs::write(good.join("inside"), b"x").unwrap();
        let missing = dir.path().join("missing");

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(
            os(&[missing.to_str().unwrap(), good.to_str().unwrap()]),
            &mut out,
            &mut err,
        );
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::from(1)));
        let stderr_text = String::from_utf8(err).unwrap();
        assert!(stderr_text.contains("missing"));
        let stdout_text = String::from_utf8(out).unwrap();
        assert!(stdout_text.contains("inside"));
    }

    #[test]
    fn stdout_write_failure_surfaces_io_error() {
        let mut out = FailingWriter;
        let mut err = Vec::new();
        let code = run(os(&["--help"]), &mut out, &mut err);
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::from(1)));
        let text = String::from_utf8(err).unwrap();
        assert!(text.contains("<stdout>"));
    }

    #[test]
    fn list_dir_write_failure_surfaces_io_error() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("x"), b"hi").unwrap();
        let mut out = FailingWriter;
        let mut err = Vec::new();
        let code = run(os(&[dir.path().to_str().unwrap()]), &mut out, &mut err);
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::from(1)));
    }

    #[test]
    fn list_file_write_failure_surfaces_io_error() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("solo");
        fs::write(&file, b"hi").unwrap();
        let mut out = FailingWriter;
        let mut err = Vec::new();
        let code = run(os(&[file.to_str().unwrap()]), &mut out, &mut err);
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::from(1)));
    }

    #[test]
    fn list_label_write_failure_surfaces_io_error() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        let mut out = FailingWriter;
        let mut err = Vec::new();
        let code = run(
            os(&[a.to_str().unwrap(), b.to_str().unwrap()]),
            &mut out,
            &mut err,
        );
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::from(1)));
    }

    #[test]
    fn list_row_trailing_newline_write_failure_surfaces_io_error() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("only"), b"hi").unwrap();
        let mut out = FailOnNewline::new(0);
        let mut err = Vec::new();
        let code = run(os(&[dir.path().to_str().unwrap()]), &mut out, &mut err);
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::from(1)));
    }

    #[test]
    fn list_separator_write_failure_surfaces_io_error() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        let mut out = FailOnNewline::new(1);
        let mut err = Vec::new();
        let code = run(
            os(&[a.to_str().unwrap(), b.to_str().unwrap()]),
            &mut out,
            &mut err,
        );
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::from(1)));
    }
}
