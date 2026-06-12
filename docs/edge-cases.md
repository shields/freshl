# Edge-case matrix

`freshl` is a pure read-only function of two inputs — a filesystem subtree and
the git state overlaying it — rendered to text. "Have we found all the edge
cases?" is really "have we searched that input space?" 100% line coverage does
_not_ answer it: every one of the recent edge-case fixes (empty untracked dirs
rendering `?`, empty dirs flagged as a dirty subtree, broken-symlink chains)
passed the line gate. They were _semantic_ gaps, not unexecuted lines.

This document enumerates the input space along every dimension we know of, so
coverage is legible. Each value is marked:

- **T** — tested (example test, and/or swept by the generative harness)
- **U** — intentionally unspecified (documented non-goal; not a bug)
- **G** — gap (no coverage; tracked in the backlog below)

The harness searches this space with oracles instead of imagination (see
`tests/`). Three suites are **generative**:

- **differential vs `git`** (`tests/differential_git.rs`) — git is the
  authoritative oracle for the status column; generated worktrees are diffed
  against `git status`/`git ls-files`.
- **property suite over a tree generator** (`tests/properties.rs`) — the
  generator knows ground truth for each entry's kind/name/size; asserts
  no-panic, determinism, well-formed aligned output, byte-faithful names.
- **bounded-exhaustive enumeration** (`tests/sort_properties.rs`) — the sort
  comparator is a strict total order, verified by enumerating a small alphabet
  exhaustively (a proof over the bound, stronger than sampling). The
  byte-oriented surfaces are small enough to cover this way rather than fuzz.

A fourth suite is hand-written:

- **targeted gap tests** (`tests/gaps.rs`) — deterministic checks for cells the
  generators can't reliably reach: special file types (FIFO/socket), submodules,
  exotic gitignore patterns, unusual repo shapes (worktree / sparse checkout /
  `core.filemode` / `core.ignorecase`), and extreme sizes/timestamps. Several
  double as differential checks against the `git` CLI.

Markers below: **T (harness)** — swept by a generative suite; **T (gap test)** —
pinned by a targeted gap test in `tests/gaps.rs`; plain **T** — a hand-written
example test beside the code it covers; **U** — intentionally unspecified.

## Entry kind (`EntryKind`, src/entry.rs)

| Value                     | Status       | Where                                                    |
| ------------------------- | ------------ | -------------------------------------------------------- |
| Regular file              | T (harness)  | pervasive                                                |
| Directory                 | T (harness)  | pervasive                                                |
| Symlink → file (resolved) | T            | lib.rs `renders_target_kind_for_symlink_to_file`         |
| Symlink → dir (resolved)  | T            | lib.rs `expands_symlink_to_directory_arg`                |
| Broken symlink            | T (harness)  | lib.rs `falls_back_on_broken_symlink`                    |
| Symlink chain (multi-hop) | T            | collect.rs follow-chain tests                            |
| Symlink cycle             | T            | lib.rs `recursive_breaks_self_referential_symlink_cycle` |
| Char device               | T            | integration.rs `/dev/null`                               |
| Block device              | T            | collect.rs `classify`; entry.rs `type_char`              |
| FIFO                      | T (gap test) | gaps.rs (`mkfifo`)                                       |
| Socket                    | T (gap test) | gaps.rs (`UnixListener`)                                 |
| "Other" / unknown         | T            | collect.rs `classify_recognises_every_posix_type`        |

## Name bytes

| Value                                   | Status       | Where                                |
| --------------------------------------- | ------------ | ------------------------------------ |
| ASCII                                   | T (harness)  | pervasive                            |
| Hidden (dot-prefix)                     | T (harness)  | lib.rs hidden-dir tests              |
| Non-UTF-8 bytes                         | T (harness)  | name byte-fidelity property          |
| Control chars (incl. ESC, newline-free) | T (harness)  | properties.rs byte-fidelity          |
| Leading/trailing whitespace             | T (harness)  | generator name pool                  |
| Very long (near `NAME_MAX`)             | T (gap test) | gaps.rs (200-byte name)              |
| Combining / RTL / emoji                 | T (harness)  | generator name pool                  |
| Embedded `/`                            | U            | impossible on POSIX (path separator) |
| NUL byte                                | U            | impossible in a filename             |

## Git status (`PorcelainCode`, src/git.rs) — files

Oracle: `git`. Swept by `tests/differential_git.rs`.

| Value                            | Status      | Where                                  |
| -------------------------------- | ----------- | -------------------------------------- |
| Clean tracked (`○`)              | T (harness) | differential + git.rs                  |
| Untracked (`?`)                  | T (harness) | differential                           |
| Ignored (`·`)                    | T (harness) | differential                           |
| Modified worktree (`●`)          | T (harness) | differential                           |
| Staged modification              | T (harness) | differential                           |
| Staged addition (`+`)            | T (harness) | differential                           |
| Deleted worktree (`▽`)           | T (harness) | differential                           |
| Staged deletion                  | T (harness) | differential                           |
| Type change (`≈`)                | T (harness) | differential                           |
| Renamed (worktree / staged, `→`) | T           | git.rs rename tests                    |
| Copied (`⇉`)                     | T           | git.rs `rewrite_code` tests            |
| Unmerged conflict (`✘`)          | T           | git.rs / integration.rs conflict tests |

## Git status — directories (freshl's own aggregation spec)

git assigns no code to a directory; these are freshl's documented refinements,
derived independently from git's per-file output in the differential test.

| Value                                          | Status      | Where                                                      |
| ---------------------------------------------- | ----------- | ---------------------------------------------------------- |
| Tracked-clean dir (`○`)                        | T (harness) | differential                                               |
| Dirty subtree (`⋯`)                            | T (harness) | differential; git.rs                                       |
| Untracked dir with content (`?`)               | T (harness) | differential                                               |
| Empty untracked dir → blank                    | T (harness) | differential; git.rs (091b30e)                             |
| Empty untracked dir doesn't flag ancestors     | T           | git.rs (377d556)                                           |
| Wholly-ignored dir (`·`)                       | T           | git.rs `.venv`-style tests                                 |
| Ignored subdir inside untracked parent         | T           | git.rs (a22d0a3)                                           |
| Deleted-tracked file + untracked content → `⋯` | T (harness) | **found & fixed** by differential; git.rs regression tests |
| Dir whose only content is ignored files (`·`)  | T (harness) | differential (ignored files nest); git.rs                  |
| Ignored files beside an empty subdir → `?`     | U           | git won't collapse it; no directory oracle                 |

## Git repo shape

| Value                                     | Status       | Where                               |
| ----------------------------------------- | ------------ | ----------------------------------- |
| No repo                                   | T (harness)  | non-git tempdirs                    |
| Nested `.git` rendered blank              | T            | git.rs `.git` tests                 |
| Submodule subtree → clean                 | T (gap test) | gaps.rs (submodule e2e)             |
| Symlinked workdir (`/var`→`/private/var`) | T            | git.rs `relativize` tests           |
| Symlinked dir argument contents           | T            | integration.rs (+ lib.rs `git_key`) |
| Symlink chain dir argument contents       | T            | integration.rs                      |
| `-d` symlinked dir with trailing slash    | T            | integration.rs (+ lib.rs `git_key`) |
| Symlinked dir target outside any repo     | T            | integration.rs                      |
| `freshl ..` / `..`-containing paths       | T            | git.rs relativize tests             |
| Bare repo (no workdir)                    | T            | snapshot cache negative path        |
| Secondary worktree (`git worktree`)       | T (gap test) | gaps.rs (linkfile + worktree index) |
| Sparse checkout                           | T (gap test) | gaps.rs (SKIP_WORKTREE ≠ deletion)  |
| `core.ignorecase` / `core.filemode=false` | T (gap test) | gaps.rs (gix honors both configs)   |

## gitignore patterns

| Value                                  | Status       | Where                           |
| -------------------------------------- | ------------ | ------------------------------- |
| Plain name / `dir/`                    | T            | recursion + git.rs tests        |
| Internal `.gitignore` of `*` (`.venv`) | T            | git.rs tests                    |
| Negation (`!keep`)                     | T (gap test) | gaps.rs (`!keep`)               |
| Globstar (`**`)                        | T (gap test) | gaps.rs (`**`)                  |
| Character class (`[ab]`)               | T (gap test) | gaps.rs (`[ab]`)                |
| Symlink-to-dir not matched by `dir/`   | T            | git.rs `is_real_dir` / `lookup` |

## Permissions & ownership

| Value                                       | Status | Where                           |
| ------------------------------------------- | ------ | ------------------------------- |
| Unreadable dir (`0o000`)                    | T      | lib.rs unreadable-dir tests     |
| No-exec dir (`0o400`, stat fails per child) | T      | lib.rs per-child-failure tests  |
| Mode == dimming default vs not              | T      | format/perms.rs tests           |
| setuid / setgid / sticky                    | T      | format/perms.rs tests           |
| Hardlinked (nlink > 1)                      | T      | format/mod.rs `dim_nlink` tests |
| gid is/ isn't owner's primary               | T      | format/mod.rs `dim_group` tests |
| Foreign uid/gid (no passwd entry)           | T      | owner.rs tests                  |
| POSIX ACLs / xattrs / SELinux               | U      | not part of the one-line layout |

## Size & time

| Value                           | Status       | Where                                        |
| ------------------------------- | ------------ | -------------------------------------------- |
| 0 bytes                         | T (harness)  | pervasive                                    |
| ≥ 1 MB (leading-digit dim)      | T            | format/size.rs tests                         |
| Near `u64::MAX`                 | T            | format/size.rs tests                         |
| Device rdev as hex              | T            | integration.rs; format/size.rs               |
| mtime now / past / dim boundary | T            | format/time.rs tests                         |
| Pre-epoch mtime                 | T            | format/time.rs `pre_epoch_renders_correctly` |
| Far-future mtime (year 2096)    | T (gap test) | gaps.rs (year 2096)                          |

## Recursion (`-R`)

| Value                           | Status      | Where                                             |
| ------------------------------- | ----------- | ------------------------------------------------- |
| Deep nesting, depth-first order | T           | lib.rs DFS tests                                  |
| Wide directory (many entries)   | T (harness) | generator                                         |
| Hidden gating (`-u`/`-uu`)      | T           | lib.rs unrestricted tests                         |
| gitignored gating (`-u`)        | T           | lib.rs `recursive_skips_gitignored_directory`     |
| Symlink cycle break             | T           | lib.rs cycle test                                 |
| Symlink to dir descended        | T           | lib.rs `recursive_descends_into_linked_directory` |
| Mount-point crossing            | U           | freshl has no `--one-file-system`                 |

## CLI surface (`args::parse`)

| Value                                    | Status      | Where                                                 |
| ---------------------------------------- | ----------- | ----------------------------------------------------- |
| Every flag, bundles, `--`, `-r` toggling | T           | args.rs tests (exhaustive)                            |
| Unknown flag → exit 2                    | T           | args.rs / lib.rs                                      |
| Multiple targets, mixed file/dir         | T           | lib.rs / integration.rs                               |
| Non-UTF-8 args                           | T (harness) | property suite                                        |
| Output to non-TTY / piped                | T           | all tests render to a `Vec`                           |
| `LS_COLORS` variations / malformed       | T           | palette.rs robustness test; exotic codes via lscolors |

## Timing / TOCTOU

| Value                                  | Status | Where                             |
| -------------------------------------- | ------ | --------------------------------- |
| Entry vanishes between stat and render | U      | AGENTS.md: read-only, unspecified |
| Concurrent git commit during walk      | U      | unspecified (stale listing)       |
| mtime crosses a boundary mid-listing   | T      | single `now` captured at startup  |

## Backlog

No open **G** cells — every dimension above is **T** or **U**. The gaps that
were here (ignored-only directories, the worktree/sparse/`filemode`/`ignorecase`
repo shapes, malformed `LS_COLORS`) are resolved; their rationale lives in the
matrix rows and the tests those rows cite, so it isn't re-listed here.

This section stays as the live mechanism, not a finished list. When the harness
flags a divergence — or a new dimension appears — add a **G** row to the
relevant table and track it here until it resolves to **T** (covered) or **U**
(intentionally unspecified). That loop is the point: the generative suites exist
to surface the _unknown_ unknowns.
