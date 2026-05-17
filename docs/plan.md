<!--
Copyright © 2026 Michael Shields

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
-->

# Implementation plan

The design is locked in [comparison.md](comparison.md). This document is the
build plan: project scaffold, module layout, dependencies, build order, and
test strategy.

## Hard requirement: 100 % test coverage

Coverage is gated, not aspirational. Every line that ships must be exercised by
a test, with `coverage(off)` permitted only on `main` (per the
`right-answers/rust.md` convention). Concretely:

- `make coverage` runs `cargo +nightly llvm-cov --cfg coverage_nightly --fail-under-lines 100`
  and is a CI-blocking job.
- A PR that drops coverage below 100 % fails the same way a lint or test
  failure does. There is no "we'll backfill later" branch.
- New code lands with its tests in the same commit. If a code path is hard to
  cover (FFI fallbacks, OS-specific branches), the design changes to make it
  injectable rather than the coverage gate getting lowered.

## 0. Scaffold

**`Cargo.toml`** — single binary crate `freshl`, edition 2024, MSRV pinned to
current stable. Lints table mirrors `right-answers/rust.md`:

```toml
[lints.rust]
unsafe_code = "forbid"
unexpected_cfgs = { level = "warn", check-cfg = ['cfg(coverage_nightly)'] }

[lints.clippy]
all      = { level = "deny", priority = -1 }
pedantic = { level = "deny", priority = -1 }
nursery  = { level = "deny", priority = -1 }
cargo    = { level = "deny", priority = -1 }
```

**`src/main.rs`** — coverage cfg-gate on `main`:

```rust
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

#[cfg_attr(coverage_nightly, coverage(off))]
fn main() -> ExitCode { … }
```

**`rust-toolchain.toml`** — stable channel for normal builds; the coverage
workflow invokes nightly explicitly (`cargo +nightly llvm-cov`).

**`Makefile`** — `build`, `test`, `lint`, `fmt`, `coverage`, `run`. `lint` runs
`cargo fmt --check` and `cargo clippy --all-targets -- -D warnings`.
`coverage` runs `cargo +nightly llvm-cov --cfg coverage_nightly
--fail-under-lines 100`.

**`lefthook.yml`** — prettier on staged `*.md`, matching `right-answers`.

**`.github/workflows/ci.yaml`** — `ubuntu-24.04`, SHA-pinned actions,
workflow-level `permissions: contents: read`, concurrency block from
`right-answers/ci.md`. Jobs: `test` (runs `make lint test`) and `coverage` (a
separate job that calls `make coverage` on the nightly toolchain). Both must
pass for merge.

**`.github/renovate.json5`** — `config:best-practices`,
`:semanticCommitsDisabled`, `platformAutomerge`, automerge for non-major and
dev deps.

**`.gitignore`** — `target/`, `*.profraw`, `lcov.info`.

**`README.md`** — one paragraph plus a link to the design doc. No
"why I built this," no installation theatre.

## 1. Dependencies (initial picks)

| Crate           | Purpose                                                |
| --------------- | ------------------------------------------------------ |
| `anstyle`       | ANSI styling primitives                                |
| `anstream`      | `NO_COLOR` / isatty-aware stdout                       |
| `terminal_size` | Width detection for the multi-path section labels      |
| `jiff`          | Epoch `mtime` → ISO 8601 UTC string                    |
| `uzers`         | uid/gid → name with numeric fallback                   |
| `gix`           | Repo discovery and status; pure Rust, no libgit2       |
| `rustix`        | `statx` (Linux casefold) and `pathconf` (macOS) probes |

`rustix` is preferred over raw `libc` / `nix` because `unsafe_code = "forbid"`
rules out direct FFI, and `rustix` exposes safe wrappers for the syscalls we
need. No `clap`, no `chrono`, no `colored`.

## 2. Module layout

```
src/
├── main.rs            entry point, top-level error reporting
├── args.rs            manual arg parsing: paths only (plus --help/--version)
├── entry.rs           Entry struct + EntryKind enum
├── collect.rs         readdir + stat → Vec<Entry>
├── owner.rs           uid/gid resolution with cache
├── git.rs             discover repo, snapshot status, lookup per-path
├── case.rs            per-FS case-sensitivity probe (macOS/Linux)
├── sort.rs            dirs-first + natural-sort comparator
├── format/
│   ├── mod.rs         Row -> String, drives column widths
│   ├── perms.rs       u32 mode → "755"
│   ├── size.rs        u64 bytes → "12345678" right-aligned, trailing
│   │                  six-digit groups dimmed for scannability
│   ├── time.rs        SystemTime → "2026-05-15T11:02:00Z" with dim T/Z
│   ├── name.rs        filename coloring by EntryKind, dim if ignored,
│   │                  symlink "name → target"
│   └── git_col.rs     porcelain pair, ✓ for tracked-clean, omitted if no repo
├── terminal.rs        width + color-cap (delegated to anstream)
└── error.rs           Error enum, exit codes
```

## 3. Build order

Each step is a small commit with tests in the same commit. Coverage stays at
100 % at every step; if a step would drop it, fix the gap before moving on.

1. **Scaffold** — `cargo init --bin`, lint table, Makefile, `main.rs`
   returning `ExitCode::SUCCESS`, CI smoke job. Verification:
   `make lint test coverage` green.
2. **Arg parsing** (`args.rs`) — manual loop over `std::env::args()`.
   Recognises `--help`, `--version`, `--`, plus positional paths. On unknown
   flag, error with exit 2 (matches `ls`).
3. **Entry collection** (`entry.rs`, `collect.rs`) — `readdir` returns
   `Vec<Entry>` with `name`, `path`, `kind`, mode, nlink, uid, gid, size,
   mtime, symlink target. Always-hidden (no filter). Uses
   `std::fs::symlink_metadata` so symlinks are not followed.
4. **Owner resolution** (`owner.rs`) — `uzers::get_user_by_uid` with a
   `HashMap<u32, OsString>` cache, numeric fallback when lookup returns
   `None`.
5. **Case detection** (`case.rs`) — `rustix::fs::pathconf(_, PC_CASE_SENSITIVE)`
   on macOS; on Linux query `statx` for `STATX_ATTR_CASE_FOLD` if the file has
   it set (default = sensitive). Cache per-directory.
6. **Sort** (`sort.rs`) — comparator: dir first, then by name. Name comparison
   is natural-order (`f2` < `f10`), case sensitivity per result of step 5.
   Implement natural-order inline (≈30 lines) rather than pull in a crate.
7. **Formatting — non-git path**:
   - `perms.rs`: `(mode & 0o777)` → `"{:o}"`.
   - `size.rs`: emit raw digits, right-aligned to the widest entry in the
     listing. Digits past the leading six-digit-aligned group are dimmed so
     the megabyte/terabyte boundary is visible without altering the text.
   - `time.rs`:
     `jiff::Timestamp::from_second(secs).strftime("%Y-%m-%dT%H:%M:%SZ")`.
     Styling: split at `T` and `Z`, dim those two characters with `anstyle`.
   - `name.rs`: ANSI color by kind. Symlinks rendered `name → target` with
     the arrow dimmed; target red if broken (lstat of target fails).
   - Column widths computed in one pass over the rows after collection.
   - `mod.rs`: stitches columns with single-space separators, in order: type ·
     mode · nlink · owner · group · size · timestamp · (git) · name.
     Right-aligns mode / nlink / size; left-aligns owner / group / name.
8. **Top-level wiring** — render to `anstream::AutoStream` so `NO_COLOR` and
   non-tty drops styling. File arg → single row. Multiple paths →
   blank-line-separated, each prefixed with a `<path>:` label line, matching
   `ls`.
9. **Git column** (`git.rs`, `format/git_col.rs`):
   - At start of a listing run, attempt `gix::discover` from each listed
     directory. Cache the result; if no repo, the column is omitted for that
     whole listing.
   - For each entry inside a repo, classify with one porcelain-equivalent
     snapshot. Map to two-char code: `M` / `A` / `D` / `R` / `C` / `T` / `U` /
     `??` / `!!`. Tracked + clean → `✓` (left column, right blank).
   - Ignored files: filename rendered with `Style::dim()` in addition to `!!`
     in the git column.
10. **Coverage hardening** — by this point coverage has been enforced at every
    step. Final pass: review any branches that needed contortion to cover, and
    refactor where the test shape is uglier than the production shape.

## 4. Tests

**Unit (in-module `#[cfg(test)]`):**

- `format::perms`: `0o755` → `"755"`; `0o644` → `"644"`; `0o7777` → `"7777"`
  (sticky / setuid only when set).
- `format::size`: 0 → `"0"`, 123 → `"123"`, 999_999 → `"999999"`,
  1_000_000 → `"1000000"` with `000000` dimmed, 1_234_567_890 →
  `"1234567890"` with `567890` dimmed, 999_999_999_999 → `"999999999999"`
  with the trailing six digits dimmed.
- `format::time`: fixed `SystemTime` epoch → expected ISO string. Test that
  `T` and `Z` get the dim style attached.
- `sort`: natural-order ordering across a fixture name list; verify dirs-first
  regardless of casing.
- `case`: behaviour-tested via a temporary HFS+ image is overkill; instead
  inject the detector with a stub and test the comparator's response to both
  modes.
- `args`: each known flag set; unknown flag → error.

**Integration (`tests/integration.rs`):**

- Build a tempdir with: regular file, hidden file, directory, broken symlink,
  valid symlink. Run the binary; assert exact output for stable columns and
  pattern-match the mtime column.
- Repeat inside a git repo (constructed via `gix`) covering: untracked,
  ignored, staged-modified, worktree-modified, tracked-clean. Assert each
  row's git column.
- Multi-path: pass two dirs; assert label lines and section separation.
- File arg: pass a single file path; assert one row, no label.
- `NO_COLOR=1`: assert no ANSI escapes anywhere.

**Coverage gate:** `--fail-under-lines 100`. `error.rs` paths covered by
negative tests (nonexistent path → exit 2). Any module that resists 100 %
gets refactored, not exempted.

## 5. Risks and open items

- **`statx` + casefold availability**: requires kernel ≥ 5.2 and a filesystem
  that reports the flag. Fallback: assume case-sensitive when the syscall does
  not return casefold info; document the assumption in `case.rs`.
- **`gix` status performance** on large repos: acceptable for v1. If it
  dominates wall time, scope to "stat the index, skip the workdir scan" later.
  Not optimising on day one.
- **Owner column width** on systems with long usernames: column auto-sizes
  per listing; no hard cap.
- **`uzers`** is a fork of the unmaintained `users` crate — confirm it still
  publishes. If not, switch to a tiny hand-rolled `getpwuid` / `getgrgid`
  wrapper via `rustix` (still no `unsafe`).

## 5a. Sort and recursion flags

After the core listing path is stable, the next layer adds the four most-used
GNU ls sort and recursion flags plus a ripgrep-style escalation modifier:

- `-S` (size, largest first) and `-t` (mtime, newest first) extend `sort.rs`
  with a `SortKey` enum and a keyed comparator. Both keep directories grouped
  first; the key only controls within-group order.
- `-r` reverses the within-group order via `Ordering::reverse` (preserving
  sort stability), still without disturbing the dirs/files split.
- `-R` introduces depth-first recursion driven from `lib.rs`. Each visited
  directory is rendered as its own labeled block. Symlinks to directories are
  never followed.
- `-u` / `-uu` gate the descent: by default `-R` skips hidden (dot-prefix)
  and gitignored directories; `-u` enables gitignored descent; `-uu` enables
  hidden descent as well. The rows are still listed in every case — only the
  descent is gated.

Short flags may be bundled (`-Rt`, `-Sr`, `-Ruu`, …). Coverage for every new
branch lands with the implementation.

## 6. First commit after approval

Scaffold only (step 0 from § 3): `cargo init`, lint table, Makefile, CI
workflow, lefthook, renovate, stub `main()`. `make lint test coverage` must
pass before the commit lands — 100 % coverage on a do-nothing binary is the
easy case, and the gate is in place from commit one. Subsequent commits each
add one numbered step from the build order.
