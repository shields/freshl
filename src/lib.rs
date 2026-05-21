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

#[cfg(not(unix))]
compile_error!("freshl targets POSIX file metadata and only builds on Unix.");

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::SystemTime;

/// `(dev, ino)` — uniquely identifies a filesystem object across mounts.
/// Used by `list_recursive` to break self-referential cycles formed by
/// directory symlinks pointing back into their own ancestor chain.
type Inode = (u64, u64);

pub mod args;
pub mod case;
pub mod collect;
pub mod entry;
pub mod error;
pub mod format;
pub mod git;
pub mod owner;
pub mod sort;

use args::{Action, ListOptions, parse};
use case::{DetectorCache, ProbeDetector};
use entry::{Entry, EntryKind};
use error::Error;
use format::palette::Palette;
use format::{Row, build_row, compute_widths, render_row};
use git::{PorcelainCode, Snapshot, SnapshotCache};
use owner::{OwnerCache, SystemDirectory};

struct Caches {
    owners: OwnerCache<SystemDirectory>,
    sensitivity: DetectorCache<ProbeDetector>,
    snapshots: SnapshotCache,
    palette: Palette,
    /// Captured once per invocation so every row's mtime is dimmed against the
    /// same reference point — no skew if a long listing crosses a minute/hour
    /// boundary mid-render.
    now: SystemTime,
    /// Process umask captured at startup, used to decide which file/dir
    /// permissions are "boring" and should be dimmed.
    umask: u32,
}

impl Caches {
    fn new() -> Self {
        Self {
            owners: OwnerCache::new(SystemDirectory),
            sensitivity: DetectorCache::new(ProbeDetector),
            snapshots: SnapshotCache::new(),
            palette: Palette::from_env(),
            now: SystemTime::now(),
            umask: read_umask(),
        }
    }
}

/// POSIX `umask(2)` only has a set-and-return form, so read the current value
/// by setting it to a known mask and immediately restoring. Safe for freshl
/// because this runs once at startup before any thread is spawned that could
/// race a concurrent `open(2)`.
// `RawMode` (the underlying type of `Mode::bits()`) is `u16` on macOS/BSD and
// `u32` on Linux; `.into()` widens uniformly. The `useless_conversion` allow
// covers the Linux identity case.
#[allow(clippy::useless_conversion)]
fn read_umask() -> u32 {
    use rustix::fs::Mode;
    use rustix::process::umask;
    let prev = umask(Mode::from_bits_truncate(0o022));
    let _ = umask(prev);
    prev.bits().into()
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

fn dispatch<I>(raw: I, stdout: &mut dyn Write, stderr: &mut dyn Write) -> Result<ExitCode, Error>
where
    I: IntoIterator<Item = OsString>,
{
    let action = parse(raw).map_err(|e| Error::Usage(e.message))?;
    match action {
        Action::Help => write_stdout(stdout, args::HELP.as_bytes()).map(|()| ExitCode::SUCCESS),
        Action::Version => write_stdout(stdout, format!("{}\n", args::version_line()).as_bytes())
            .map(|()| ExitCode::SUCCESS),
        Action::List { paths, options } => list(stdout, stderr, &paths, options),
    }
}

fn write_stdout(stdout: &mut dyn Write, bytes: &[u8]) -> Result<(), Error> {
    stdout.write_all(bytes).map_err(stdout_io)
}

fn list(
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    paths: &[PathBuf],
    options: ListOptions,
) -> Result<ExitCode, Error> {
    let fallback = [PathBuf::from(".")];
    let targets: &[PathBuf] = if paths.is_empty() { &fallback } else { paths };
    // Under -R every directory gets a label so each block is identifiable in
    // the depth-first stream; otherwise only multi-target listings label.
    let label_dirs = options.recursive || targets.len() > 1;
    let mut had_error = false;
    let mut caches = Caches::new();

    // Split into a batch of files and a list of directories. Files render
    // together so column widths span all file arguments (matching `ls -l
    // file1 file2 …`); each directory then renders as its own block with
    // its own widths. With -d, directories are not expanded — they all go
    // into the files batch so they render as plain rows alongside any file
    // arguments.
    let mut files: Vec<Entry> = Vec::new();
    let mut dirs: Vec<Entry> = Vec::new();
    for target in targets {
        match collect::entry_for_path(target) {
            Ok(mut entry) => {
                // Display the user-supplied path so a `freshl /etc/passwd`
                // row reads `… /etc/passwd`, not just `passwd`, and so the
                // dir labels under -R / multi-target match what was typed.
                entry.name = target.as_os_str().to_os_string();
                if entry.kind == EntryKind::Directory && !options.directory {
                    dirs.push(entry);
                } else {
                    files.push(entry);
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
    // Apply the requested sort key to top-level CLI args within each split.
    // No filesystem to probe for top-level args (they can span filesystems);
    // Sensitive is the natural default and only matters for the natural-name
    // tie-breaker.
    sort::sort_with(
        &mut files,
        case::Sensitivity::Sensitive,
        options.sort_key,
        options.reverse,
    );
    sort::sort_with(
        &mut dirs,
        case::Sensitivity::Sensitive,
        options.sort_key,
        options.reverse,
    );

    let mut have_output = !files.is_empty();
    if have_output {
        render_files(stdout, &files, &mut caches)?;
    }
    for dir_entry in &dirs {
        if have_output {
            writeln!(stdout).map_err(stdout_io)?;
        }
        let target = &dir_entry.path;
        let result = if options.recursive {
            list_recursive(stdout, stderr, target, options, &mut caches)
        } else {
            list_directory(stdout, stderr, target, label_dirs, options, &mut caches)
        };
        match result {
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
    options: ListOptions,
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
    sort::sort_with(&mut entries, sense, options.sort_key, options.reverse);
    let snapshot = caches.snapshots.for_target(target);
    render_entries(
        stdout,
        &entries,
        &mut caches.owners,
        &caches.palette,
        snapshot,
        caches.now,
        caches.umask,
    )?;
    Ok(report_listing_errors(stderr, &listing.errors))
}

fn report_listing_errors(stderr: &mut dyn Write, errors: &[(PathBuf, io::Error)]) -> bool {
    for (path, source) in errors {
        let _ = writeln!(
            stderr,
            "{}",
            Error::Io {
                path: path.clone(),
                source: io::Error::new(source.kind(), source.to_string()),
            }
        );
    }
    !errors.is_empty()
}

/// Walk `root` depth-first, rendering each directory as its own labeled
/// block. Subdirectory descent is gated on the unrestricted level: hidden
/// (dot-prefix) and gitignored directories are skipped by default and
/// progressively un-skipped at `-u` (gitignored) and `-uu` (hidden too).
///
/// Symlinks-to-directories are reclassified as `Directory` by
/// `entry_for_path` and descended into like real dirs. A per-path
/// ancestor-inode set keeps a symlink that resolves back into its own
/// ancestor chain from forming an infinite loop; non-ancestor revisits
/// (two siblings linking to the same target) are still listed, matching
/// `ls -LR`.
fn list_recursive(
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    root: &Path,
    options: ListOptions,
    caches: &mut Caches,
) -> Result<bool, Error> {
    let mut stack: VecDeque<(PathBuf, Vec<Inode>)> = VecDeque::new();
    // Best-effort: a stat failure here would also fail `collect_directory`
    // below, where the error is already reported with full context.
    let root_ancestors = std::fs::metadata(root)
        .ok()
        .map(|m| vec![(m.dev(), m.ino())])
        .unwrap_or_default();
    stack.push_back((root.to_path_buf(), root_ancestors));
    let mut had_error = false;
    let mut first = true;
    while let Some((target, ancestors)) = stack.pop_front() {
        let listing = match collect::collect_directory(&target) {
            Ok(listing) => listing,
            Err(source) => {
                // If the root itself fails, surface the error to the caller
                // exactly like the non-recursive `list_directory` does — that
                // way the outer `list` loop can keep `have_output` correct
                // and not emit a blank-line separator for an empty block.
                if first {
                    return Err(Error::Io {
                        path: target,
                        source,
                    });
                }
                let _ = writeln!(
                    stderr,
                    "{}",
                    Error::Io {
                        path: target.clone(),
                        source,
                    }
                );
                had_error = true;
                continue;
            }
        };
        // Separator goes between *rendered* blocks; failed targets above
        // don't count, so we don't leave an orphan blank line behind them.
        if !first {
            writeln!(stdout).map_err(stdout_io)?;
        }
        first = false;
        write_path_with_suffix(stdout, &target, b":\n")?;
        let mut entries = listing.entries;
        let sense = {
            let names: Vec<&std::ffi::OsStr> = entries.iter().map(|e| e.name.as_os_str()).collect();
            caches.sensitivity.sensitivity(&target, &names)
        };
        sort::sort_with(&mut entries, sense, options.sort_key, options.reverse);
        let snapshot = caches.snapshots.for_target(&target);
        // Decide descent BEFORE rendering so we don't have to revisit the
        // snapshot lookup later (it can canonicalize, so each call has cost).
        let mut to_push: Vec<(PathBuf, Vec<Inode>)> = Vec::new();
        for entry in &entries {
            if entry.kind == EntryKind::Directory && should_descend(entry, snapshot, options) {
                let key = (entry.dev, entry.ino);
                if ancestors.contains(&key) {
                    continue;
                }
                let mut child_ancestors = ancestors.clone();
                child_ancestors.push(key);
                to_push.push((entry.path.clone(), child_ancestors));
            }
        }
        render_entries(
            stdout,
            &entries,
            &mut caches.owners,
            &caches.palette,
            snapshot,
            caches.now,
            caches.umask,
        )?;
        had_error |= report_listing_errors(stderr, &listing.errors);
        // Push in reverse so the first sorted subdir pops next: depth-first
        // in the order rendered, matching GNU `ls -R`.
        for child in to_push.into_iter().rev() {
            stack.push_front(child);
        }
    }
    Ok(had_error)
}

fn should_descend(entry: &Entry, snapshot: Option<&Snapshot>, options: ListOptions) -> bool {
    let is_hidden = entry.name.as_bytes().first() == Some(&b'.');
    if is_hidden && options.unrestricted < 2 {
        return false;
    }
    if options.unrestricted < 1
        && snapshot.is_some_and(|s| s.lookup(&entry.path) == PorcelainCode::IGNORED)
    {
        return false;
    }
    true
}

fn render_entries(
    stdout: &mut dyn Write,
    entries: &[Entry],
    owners: &mut OwnerCache<SystemDirectory>,
    palette: &Palette,
    snapshot: Option<&Snapshot>,
    now: SystemTime,
    umask: u32,
) -> Result<(), Error> {
    let mut rows: Vec<Row> = entries
        .iter()
        .map(|e| build_row(e, owners, palette, now, umask))
        .collect();
    for (row, entry) in rows.iter_mut().zip(entries.iter()) {
        enrich_row(row, entry, palette, snapshot);
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
    let now = caches.now;
    let umask = caches.umask;
    for entry in entries {
        let mut row = build_row(entry, &mut caches.owners, &caches.palette, now, umask);
        let snap = caches.snapshots.for_target(&entry.path);
        if snap.is_some() {
            any_git = true;
        }
        enrich_row(&mut row, entry, &caches.palette, snap);
        rows.push(row);
    }
    let git_width = if any_git { format::git_col::WIDTH } else { 0 };
    write_rows(stdout, &rows, git_width)
}

fn enrich_row(row: &mut Row, entry: &Entry, palette: &Palette, snapshot: Option<&Snapshot>) {
    let code = snapshot
        .map(|s| s.display_code_for(&entry.path, entry.kind == EntryKind::Directory));
    if let Some(c) = code {
        row.git = Some(format::git_col::render(c));
    }
    let ignored = code == Some(PorcelainCode::IGNORED);
    let missing = entry.kind == EntryKind::Symlink && target_is_missing(entry);
    if ignored || missing {
        row.name = format::name::format_name(palette, entry, ignored, missing);
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
fn write_path_with_suffix(stdout: &mut dyn Write, path: &Path, suffix: &[u8]) -> Result<(), Error> {
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
        // Match kind+mode bytes only; the row may include surrounding ANSI escapes.
        assert!(text.contains(" 644"));
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
            dev: 0,
            ino: 0,
            follow_chain: Vec::new(),
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
            dev: 0,
            ino: 0,
            follow_chain: Vec::new(),
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
            dev: 0,
            ino: 0,
            follow_chain: Vec::new(),
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

    #[test]
    fn recursive_label_write_failure_surfaces_io_error() {
        // Exercises the `?` on write_path_with_suffix in list_recursive.
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("only"), b"hi").unwrap();
        let mut out = FailingWriter;
        let mut err = Vec::new();
        let code = run(
            os(&["-R", dir.path().to_str().unwrap()]),
            &mut out,
            &mut err,
        );
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::from(1)));
    }

    #[test]
    fn recursive_render_failure_surfaces_io_error() {
        // The label write succeeds (one newline budget) but the first row
        // newline trips FailOnNewline, exercising the `?` on render_entries
        // inside list_recursive — the non-recursive path has its own test.
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("only"), b"hi").unwrap();
        let mut out = FailOnNewline::new(1);
        let mut err = Vec::new();
        let code = run(
            os(&["-R", dir.path().to_str().unwrap()]),
            &mut out,
            &mut err,
        );
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::from(1)));
    }

    #[test]
    fn recursive_separator_write_failure_surfaces_io_error() {
        // First iteration writes label + 1 row (2 newlines). Second iteration
        // hits the inter-block separator writeln (3rd newline) and fails,
        // exercising the `?` on the separator inside list_recursive.
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        let mut out = FailOnNewline::new(2);
        let mut err = Vec::new();
        let code = run(
            os(&["-R", dir.path().to_str().unwrap()]),
            &mut out,
            &mut err,
        );
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::from(1)));
    }

    #[test]
    fn recursive_unreadable_root_surfaces_io_error() {
        // Exercises the `first` branch in list_recursive that returns Err
        // when the root itself fails to read on the first iteration.
        use std::os::unix::fs::PermissionsExt;
        // Restore perms on drop so an assert panic doesn't leak a 0o000
        // directory that tempdir's cleanup can't remove.
        struct Restore(std::path::PathBuf);
        impl Drop for Restore {
            fn drop(&mut self) {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
            }
        }
        let dir = tempdir().unwrap();
        let locked = dir.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let _restore = Restore(locked.clone());
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(os(&["-R", locked.to_str().unwrap()]), &mut out, &mut err);
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::from(1)));
    }

    #[test]
    fn recursive_lists_nested_directories_depth_first_with_labels() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a");
        let b = a.join("b");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        fs::write(a.join("leaf"), b"x").unwrap();
        fs::write(b.join("deep"), b"y").unwrap();

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(
            os(&["-R", dir.path().to_str().unwrap()]),
            &mut out,
            &mut err,
        );
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
        let text = String::from_utf8(out).unwrap();
        let root_label = format!("{}:", dir.path().display());
        let a_label = format!("{}:", a.display());
        let b_label = format!("{}:", b.display());
        let root_at = text.find(&root_label).expect("root label present");
        let a_at = text.find(&a_label).expect("a label present");
        let b_at = text.find(&b_label).expect("b label present");
        assert!(root_at < a_at, "root must precede a:\n{text}");
        assert!(a_at < b_at, "a must precede b (depth first):\n{text}");
        assert!(text.contains("leaf"));
        assert!(text.contains("deep"));
    }

    #[test]
    fn recursive_skips_hidden_directory_by_default_but_lists_it() {
        let dir = tempdir().unwrap();
        let hidden = dir.path().join(".secret");
        fs::create_dir(&hidden).unwrap();
        fs::write(hidden.join("inside"), b"x").unwrap();

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(
            os(&["-R", dir.path().to_str().unwrap()]),
            &mut out,
            &mut err,
        );
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
        let text = String::from_utf8(out).unwrap();
        // The hidden directory row is listed (always-hidden rule), but its
        // contents are NOT recursed into.
        assert!(text.contains(".secret"));
        assert!(!text.contains("inside"), "should not recurse: {text}");
    }

    #[test]
    fn double_unrestricted_recurses_into_hidden_directories() {
        let dir = tempdir().unwrap();
        let hidden = dir.path().join(".secret");
        fs::create_dir(&hidden).unwrap();
        fs::write(hidden.join("inside"), b"x").unwrap();

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(
            os(&["-Ruu", dir.path().to_str().unwrap()]),
            &mut out,
            &mut err,
        );
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("inside"));
    }

    #[test]
    fn recursive_reports_subdirectory_error_and_continues() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let good = dir.path().join("good");
        let locked = dir.path().join("locked");
        fs::create_dir(&good).unwrap();
        fs::create_dir(&locked).unwrap();
        fs::write(good.join("inside"), b"x").unwrap();
        let mut p = fs::metadata(&locked).unwrap().permissions();
        p.set_mode(0o000);
        fs::set_permissions(&locked, p).unwrap();

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(
            os(&["-R", dir.path().to_str().unwrap()]),
            &mut out,
            &mut err,
        );

        let mut p = fs::metadata(&locked).unwrap().permissions();
        p.set_mode(0o755);
        fs::set_permissions(&locked, p).unwrap();

        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::from(1)));
        let err_text = String::from_utf8(err).unwrap();
        assert!(err_text.contains("locked"));
        let out_text = String::from_utf8(out).unwrap();
        assert!(
            out_text.contains("inside"),
            "sibling content still rendered: {out_text}"
        );
    }

    #[test]
    fn recursive_skips_gitignored_directory_by_default() {
        use std::process::Command;
        let dir = tempdir().unwrap();
        // Set up a tiny repo with an ignored subdirectory.
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "t"],
        ] {
            let status = Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("HOME", dir.path())
                .status()
                .unwrap();
            assert!(status.success());
        }
        let ignored = dir.path().join("ignored_dir");
        fs::create_dir(&ignored).unwrap();
        fs::write(ignored.join("buried"), b"x").unwrap();
        fs::write(dir.path().join(".gitignore"), b"ignored_dir/\n").unwrap();

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(
            os(&["-R", dir.path().to_str().unwrap()]),
            &mut out,
            &mut err,
        );
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("ignored_dir"), "row still listed: {text}");
        assert!(
            !text.contains("buried"),
            "must not recurse into ignored: {text}"
        );

        // With -Ru we should descend into the gitignored directory.
        let mut out2 = Vec::new();
        let mut err2 = Vec::new();
        let code2 = run(
            os(&["-Ru", dir.path().to_str().unwrap()]),
            &mut out2,
            &mut err2,
        );
        assert_eq!(code_repr(code2), code_repr(std::process::ExitCode::SUCCESS));
        let text2 = String::from_utf8(out2).unwrap();
        assert!(text2.contains("buried"), "-Ru must recurse: {text2}");
    }

    #[test]
    fn recursive_reverse_keeps_dfs_between_blocks() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        fs::write(a.join("inner"), b"x").unwrap();
        fs::write(b.join("inner"), b"y").unwrap();

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(
            os(&["-Rr", dir.path().to_str().unwrap()]),
            &mut out,
            &mut err,
        );
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
        let text = String::from_utf8(out).unwrap();
        let a_label = format!("{}:", a.display());
        let b_label = format!("{}:", b.display());
        let a_at = text.find(&a_label).unwrap();
        let b_at = text.find(&b_label).unwrap();
        // Within the root block, -r reverses → b row precedes a row → b: block
        // is visited first when we pop the DFS stack.
        assert!(b_at < a_at, "reverse should put b: before a:\n{text}");
    }

    #[test]
    fn sort_by_size_puts_largest_at_bottom() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("small"), b"x").unwrap();
        fs::write(dir.path().join("big"), vec![b'x'; 5_000]).unwrap();
        fs::write(dir.path().join("mid"), vec![b'x'; 500]).unwrap();

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(
            os(&["-S", dir.path().to_str().unwrap()]),
            &mut out,
            &mut err,
        );
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
        let text = String::from_utf8(out).unwrap();
        let small_at = text.find("small").unwrap();
        let mid_at = text.find("mid").unwrap();
        let big_at = text.find("big").unwrap();
        assert!(small_at < mid_at && mid_at < big_at, "order:\n{text}");
    }

    #[test]
    fn recursive_per_child_stat_failure_is_reported_and_continues() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let inner = dir.path().join("inner");
        fs::create_dir(&inner).unwrap();
        fs::write(inner.join("child"), b"hi").unwrap();
        // r-- on the directory itself: readdir returns names, but `lstat` of
        // each child fails because of the missing +x bit. That feeds the
        // `listing.errors` accumulation in list_recursive.
        let mut p = fs::metadata(&inner).unwrap().permissions();
        p.set_mode(0o400);
        fs::set_permissions(&inner, p).unwrap();

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(
            os(&["-R", dir.path().to_str().unwrap()]),
            &mut out,
            &mut err,
        );

        let mut p = fs::metadata(&inner).unwrap().permissions();
        p.set_mode(0o755);
        fs::set_permissions(&inner, p).unwrap();

        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::from(1)));
        let err_text = String::from_utf8(err).unwrap();
        assert!(
            err_text.contains("child"),
            "error mentions child: {err_text}"
        );
    }

    #[test]
    fn file_arg_inside_git_repo_shows_git_column() {
        use std::process::Command;
        let dir = tempdir().unwrap();
        for cmd_args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "t"],
        ] {
            let status = Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(cmd_args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("HOME", dir.path())
                .status()
                .unwrap();
            assert!(status.success());
        }
        let file = dir.path().join("tracked");
        fs::write(&file, b"hi").unwrap();
        let status = Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["add", "tracked"])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("HOME", dir.path())
            .status()
            .unwrap();
        assert!(status.success());

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(os(&[file.to_str().unwrap()]), &mut out, &mut err);
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
        let text = String::from_utf8(out).unwrap();
        // Staged addition gets `A `; covers the `any_git = true` branch in
        // render_files.
        assert!(
            text.contains('A'),
            "expected git column for staged add: {text}"
        );
    }

    #[test]
    fn renders_target_kind_for_symlink_to_file() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("target");
        fs::write(&target, b"contents").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(os(&[link.to_str().unwrap()]), &mut out, &mut err);
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains(link.to_str().unwrap()));
        assert!(
            text.contains('→'),
            "symlink should render with forward arrow: {text}"
        );
    }

    #[test]
    fn renders_chain_forward_to_target_name() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("AGENTS.md"), b"x").unwrap();
        std::os::unix::fs::symlink("AGENTS.md", dir.path().join("CLAUDE.md")).unwrap();

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(os(&[dir.path().to_str().unwrap()]), &mut out, &mut err);
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
        let text = String::from_utf8(out).unwrap();
        // Each segment carries its own ANSI style with explicit resets, so
        // the name/arrow/target are sandwiched between control sequences
        // rather than forming a contiguous substring. Find the symlink
        // row's CLAUDE.md and confirm an arrow + AGENTS.md follow it.
        assert!(text.contains('→'), "no arrow: {text}");
        let arrow = text.find('→').unwrap();
        let pre = &text[..arrow];
        let post = &text[arrow..];
        assert!(
            pre.contains("CLAUDE.md"),
            "link name must precede arrow: {text}"
        );
        assert!(
            post.contains("AGENTS.md"),
            "target must follow arrow: {text}"
        );
    }

    #[test]
    fn expands_symlink_to_directory_arg() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("real");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("inside"), b"x").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(os(&[link.to_str().unwrap()]), &mut out, &mut err);
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("inside"), "expanded contents: {text}");
    }

    #[test]
    fn falls_back_on_broken_symlink() {
        let dir = tempdir().unwrap();
        let link = dir.path().join("dangling");
        std::os::unix::fs::symlink(dir.path().join("nope"), &link).unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(os(&[dir.path().to_str().unwrap()]), &mut out, &mut err);
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("dangling"));
        assert!(text.contains('→'), "broken link still shows arrow: {text}");
    }

    #[test]
    fn recursive_descends_into_linked_directory() {
        let dir = tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir(&real).unwrap();
        fs::write(real.join("inside"), b"x").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(
            os(&["-R", dir.path().to_str().unwrap()]),
            &mut out,
            &mut err,
        );
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
        let text = String::from_utf8(out).unwrap();
        let link_label = format!("{}:", link.display());
        assert!(
            text.contains(&link_label),
            "symlink dir should be descended into: {text}"
        );
    }

    #[test]
    fn recursive_breaks_self_referential_symlink_cycle() {
        let dir = tempdir().unwrap();
        let inner = dir.path().join("inner");
        fs::create_dir(&inner).unwrap();
        let cycle = inner.join("loop");
        std::os::unix::fs::symlink(&inner, &cycle).unwrap();

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(
            os(&["-R", dir.path().to_str().unwrap()]),
            &mut out,
            &mut err,
        );
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
        let text = String::from_utf8(out).unwrap();
        // The cycle link is listed once (under inner:) but its own block
        // (which would re-render inner's contents) must not appear.
        let loop_label = format!("{}:", cycle.display());
        assert!(
            !text.contains(&loop_label),
            "self-loop must not produce its own block: {text}"
        );
    }

    #[test]
    fn directory_flag_lists_directory_itself_not_contents() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("inside"), b"x").unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(
            os(&["-d", dir.path().to_str().unwrap()]),
            &mut out,
            &mut err,
        );
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 1, "expected one row: {text}");
        assert!(text.contains(dir.path().to_str().unwrap()));
        assert!(!text.contains("inside"), "should not list contents: {text}");
    }

    #[test]
    fn directory_flag_with_recursive_does_not_recurse() {
        let dir = tempdir().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("deep"), b"x").unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(
            os(&["-dR", dir.path().to_str().unwrap()]),
            &mut out,
            &mut err,
        );
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 1, "expected one row: {text}");
        assert!(!text.contains("deep"), "must not recurse with -d: {text}");
        assert!(!text.contains("sub"), "must not list children: {text}");
    }

    #[test]
    fn directory_flag_mixes_files_and_dirs_in_one_block() {
        let dir = tempdir().unwrap();
        let sub = dir.path().join("sub");
        let file = dir.path().join("file");
        fs::create_dir(&sub).unwrap();
        fs::write(&file, b"x").unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(
            os(&["-d", sub.to_str().unwrap(), file.to_str().unwrap()]),
            &mut out,
            &mut err,
        );
        assert_eq!(code_repr(code), code_repr(std::process::ExitCode::SUCCESS));
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 2, "expected two rows: {text}");
        // No `<path>:` label lines under -d — both args render as plain rows
        // with shared widths, like a multi-file `ls -l`.
        assert!(
            !text.lines().any(|l| l.ends_with(':')),
            "no labels expected: {text}"
        );
    }
}
