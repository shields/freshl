# Survey of `ls` and its replacements

A review of existing file-listing tools, to inform what `freshl` should and
shouldn't do. Configurability is a non-goal for `freshl`: this survey is here
to pick winners on defaults, not to catalog flags.

## The incumbents

### GNU `ls` (coreutils)

The reference implementation. Distributed with virtually every Linux system.

- **Default output**: bare names, one entry per file, column-packed to terminal
  width. No color unless `--color=auto` (most distros alias it in).
- **Long mode (`-l`)**: mode, link count, owner, group, size in bytes,
  abbreviated mtime, name. mtime year is dropped if "recent" (<6 months),
  added otherwise — surprisingly confusing.
- **Sorting**: alphabetical by default. Configurable via `-t`, `-S`, `-X`,
  `-v` (version sort), and reversible with `-r`.
- **Hidden files**: hidden unless `-a` or `-A`.
- **Strengths**: ubiquitous, fast, scriptable, stable. `--time-style=full-iso`
  gives unambiguous timestamps.
- **Weaknesses**: defaults are tuned for 1971. Sizes in bytes by default.
  Permissions as a 10-character mode string. No git awareness. No icons. No
  hyperlinks. Color scheme relies on the obscure `LS_COLORS` DSL.

### BSD `ls` (macOS, FreeBSD)

Ships on macOS. Different flag vocabulary from GNU; same era of taste.

- **Color**: `-G` flag (vs GNU's `--color=auto`), driven by `LSCOLORS` (vs
  `LS_COLORS`).
- **Time formatting**: `-T` to show full time; **no `--time-style`** equivalent.
- **Missing vs GNU**: no `--group-directories-first`, no `--ignore-backups`,
  no `-v` natural sort.
- **Extras**: `-o` shows BSD file flags in long mode.
- **Strengths**: ships everywhere Apple/BSD does.
- **Weaknesses**: all of GNU's, plus less flexible time formatting.

## The modern alternatives

### exa (deprecated)

The Rust-based pioneer. Last release 2021; the repo carries a notice pointing
to eza. Listed here for completeness — don't use it.

Design moves worth remembering:
- Color-coded permissions, with a separate color per bit.
- Git status column in long mode.
- Tree view as a first-class option (`-T`), not a separate `tree` binary.
- Color-scaled sizes (bigger files redder).

### eza

The community fork of exa, actively maintained. Currently the strongest
all-rounder. Single Rust binary.

- **Default output**: colored grid, file-type colors, optional icons (off by
  default; `--icons` or `--icons=auto`).
- **Long mode**: permissions, size (human-readable), user, modified, name.
  Skips link count and group by default — a deliberate trim of the noisiest
  GNU columns.
- **Git integration**: `--git` adds a status column; `--git-repos` shows the
  HEAD branch when listing repo roots.
- **Time**: relative-time mode (`--time-style=relative`) shows "2 days ago"
  style. ISO and custom strftime also supported.
- **Tree**: `-T` with `--level=N` cap.
- **Hyperlinks**: OSC 8 terminal hyperlinks via `--hyperlink`.
- **Recent additions**: mount-point details, SELinux context, bright terminal
  colors, theme.yml for color/icon customization.
- **Strengths**: sensible defaults, mature, fast, broad feature surface.
- **Weaknesses**: still a thin wrapper over the ls mental model — a grid of
  names, a long mode with the same columns everyone uses. Icons require a
  Nerd Font. Configuration surface has grown large (theme files, env vars,
  many flags).

### lsd (LSDeluxe)

Another Rust rewrite, inspired by Ruby's colorls. Comparable feature surface
to eza, slightly different taste.

- **Default output**: colored grid with icons **on by default** (assumes Nerd
  Font present). This is the loudest first impression of any tool here.
- **Long mode**: similar columns to eza; sizes split into number and unit
  with separate coloring.
- **Tree**: `--tree`, with `--depth`.
- **Git**: `--git` flag added relatively late; less polished than eza's.
- **Configuration**: three YAML files (`config.yaml`, `colors.yaml`,
  `icons.yaml`) in XDG locations.
- **Strengths**: arguably the prettiest out of the box. Cross-platform
  including Windows.
- **Weaknesses**: defaults assume Nerd Font, which silently degrades to
  tofu without one. Heavier reliance on icons to communicate type; less
  information density in the long view.

## The structural alternatives

These don't try to be `ls`. They're worth noting because they show different
mental models worth stealing from.

### nushell `ls`

Built into Nushell. Returns a table of structured records — name, type, size,
modified — that pipes into other Nushell commands as data, not text.

- **Strength**: the columnar long view is the *only* view. No grid/long
  dichotomy. Sortable, filterable, projectable via the shell's data
  pipeline.
- **Lesson**: even a CLI tool can lean further into "table of files" over
  "grid of names."

### nuls

A standalone Rust binary that mimics Nushell's table output without requiring
Nushell. Recency-colored relative modified times, type tags, human sizes.

- **Lesson**: there's appetite for "always show me the table" as a default,
  not as `-l`.

### broot

Interactive tree navigator. Type-to-filter, preview pane, git integration.
Replaces `tree` more than `ls`.

- **Lesson**: in a TTY, "show me a tree, let me filter" is often what you
  actually wanted. Not relevant to a non-interactive `ls` replacement, but
  worth keeping in mind for what `freshl` *won't* try to be.

### tree

The classic. Recursive ASCII tree. Still useful, still installed
everywhere. Every modern alternative absorbed it as a `-T` flag.

## Synthesis: what each tool gets right

| Tool      | Best idea worth keeping                                              |
| --------- | -------------------------------------------------------------------- |
| GNU ls    | Be fast, be a single binary, be scriptable                           |
| BSD ls    | (nothing distinctive — has been overtaken)                           |
| exa       | Per-bit permission colors, color-scaled sizes                        |
| eza       | ISO time mode (`--time-style=iso`); OSC 8 hyperlinks                 |
| lsd       | Visual hierarchy via consistent type/size coloring                   |
| nushell   | "Table of files" as the *only* model, not as a `-l` mode             |
| nuls      | Type tags over icons for type identification                         |
| broot     | Trees beat grids when there's more than a screenful                  |
| tree      | Tree drawing is a solved problem; copy it                            |

## Synthesis: what they get wrong

- **Two output modes** (grid + long). Almost every tool above asks the user
  to choose between "names only" and "everything." Real usage wants
  something in between, every time. Pick one good default.
- **Icons over typography.** Both eza (opt-in) and lsd (default) lean on
  Nerd Font glyphs to communicate type. This breaks in any terminal without
  the font, in any pipe, in any screenshot shared with a colleague. Color
  and text tags survive these contexts; icons don't.
- **Unreadable byte counts.** GNU/BSD ls print `1234567890` as an
  undelimited string of digits, leaving the reader to count digits to
  judge the magnitude. The "modern" tools mostly fixed this by switching
  to `1.2G`-style units, which trades one problem (digit-counting) for
  another (loss of precision). Bytes are the right unit; the formatting
  is the bug.
- **mtime ambiguity.** "Jun 14 09:32" is unparseable without knowing the
  current year, and the relative-time mode ("3 days ago") trades one
  ambiguity for another — you still don't know the absolute time, and
  the answer changes depending on when you read it. ISO 8601 is the
  only format that doesn't lie.
- **Local-time timestamps.** Every tool here defaults to the system's
  local timezone, without marking it as such. Copy a listing into a bug
  report, paste it into a chat, share it with a colleague in another
  zone, and the timestamps become misleading. UTC with an explicit `Z`
  removes the ambiguity.
- **Configurability as a goal.** eza ships a theme.yml, three color
  environment variables, and dozens of flags. lsd ships three YAML files.
  This is a tax: every user spends time tuning, and every user's terminal
  looks different. Pick defaults and commit.
- **Permission strings.** `-rwxr-xr-x` is a 10-character glyph that requires
  bit-counting to read. Octal (`755`) is shorter and at least as legible
  for anyone who actually reads them.

## Implications for `freshl`

Drawing from the above, the design `freshl` commits to:

### Output

1. **One output mode.** A single, opinionated layout that always shows
   what's worth showing. No `-l` toggle. This is "ls but better," not
   a reimagining — keep the full classic column set: type, mode, link
   count, owner, group, size, mtime, name.
2. **Type prefix retained** as a single character at the very left of
   each row (`d` directory, `l` symlink, `-` regular file, plus the
   usual `c`/`b`/`p`/`s`). Familiar from `ls -l`; cheap to read.
3. **Permissions in octal** (`755`, no leading zero). Drops the
   bit-counting load of `rwxr-xr-x`; three digits beat nine glyphs.
4. **Type by color** on the filename in addition to the type prefix.
   No icons.
5. **ISO 8601 UTC timestamps**, with an explicit `Z` suffix. Always.
   No relative dates, no local-time guessing, no year-omission. The
   timestamp should mean the same thing whether you read it now, next
   year, or in a chat log shared by a colleague three time zones away.
   The `T` separator and `Z` suffix are rendered dim; the digits get
   full contrast, so the eye lands on the part that varies.
6. **Raw byte counts**, emitted as a plain integer (copy/paste works,
   no separator to strip). Digits past the leading six-digit-aligned
   group are dimmed (e.g. `12345678` → `12` bright, `345678` dim;
   `1234567890` → `1234` bright, `567890` dim), so the megabyte
   boundary is visible without altering the text. Right-aligned in
   the column.
7. **Symlinks** displayed on the same row as `name → target`, with the
   arrow and target distinctly colored (and dim/red if the target is
   missing).
8. **Owner and group** as names, with numeric uid/gid as a fallback
   when name resolution fails (NFS, deleted accounts, container
   environments with no passwd entries).
9. **Time column shows mtime only.** No atime, no ctime, no birthtime.
   If you need another, there are other tools.

### Behavior

10. **Always show hidden files.** No `-a` flag, no toggle. If a file is
    in the directory, it's in the listing.
11. **Sort**: directories first, then everything else. Within each
    group, natural sort (`f2` before `f10`). Case sensitivity follows
    the filesystem — macOS via `pathconf(_PC_CASE_SENSITIVE)`, Linux
    defaults to case-sensitive (with `statx` casefold detection where
    the kernel supports it). `-S` (size, largest first) and `-t`
    (mtime, newest first) change the within-group key but keep
    directories grouped first; `-r` reverses the within-group order
    while leaving the dirs/files split intact. Top-level CLI
    arguments are sorted by the same rules within their file batch
    and their directory list.
12. **Multiple path arguments** emit each as a separate labeled
    section, like `ls foo/ bar/`.
13. **A file argument** prints a single row for that file. No
    directory traversal.
14. **`-R`** walks directories depth-first, one labeled block per
    directory visited. Symlinks to directories are never followed
    (cycles impossible, behaviour stable). By default, recursion
    skips hidden directories (dot-prefixed names) and gitignored
    directories — the rows are still listed, only the descent is
    gated. The gate is opened ripgrep-style: `-u` also recurses into
    gitignored directories; `-uu` also recurses into hidden ones.
15. **Targets**: Linux and macOS only. No Windows, no BSDs (initially).

### Git integration

15. **Two-character git status column**, matching `git status
    --porcelain` with one addition. First char = index, second =
    worktree. Symbols: `M` modified, `A` added, `D` deleted, `R`
    renamed, `C` copied, `T` type-change, `U` unmerged. `??` for
    untracked and `!!` for ignored (both positions). The one
    extension to porcelain: tracked-and-clean renders as `✓` in
    the index position (rather than two blanks), so the column
    distinguishes "git knows this file and it's clean" from "we're
    not in a git repo and there's no column." Field separators
    throughout the row are a single space.
16. **Column placement**: adjacent to the filename (right of the
    metadata columns), not far-left — git status describes the file,
    so it reads better next to the name.
17. **Column appears only when relevant.** If the listed directory is
    not inside a git work tree, the column is omitted entirely
    rather than rendered as blanks.
18. **Ignored files** get a dimmed filename in addition to the `!!`
    status, so they fade visually even when scanning by name alone.

### Non-goals

19. **Zero config files.** Environment-respect (`NO_COLOR`, terminal
    width) yes; user config no. No themes, no YAML, no env-var color
    DSLs.
20. **No tree-drawing recursion** (for now). `-R` lists nested
    directories as labeled blocks; ASCII tree art with branch glyphs
    is `tree`/`broot` territory and would be a subcommand if added.
21. **No OSC 8 hyperlinks.** Don't decorate filenames with terminal
    escape sequences that some clients render and others leak as
    visual noise.

### Implementation

22. **Rust.** Single static binary. Specific crate choices deferred
    until coding starts.

### Sample output

What a listing in this design looks like (git column shown because
the example is inside a repo; `T` and `Z` would be dimmed in a real
terminal):

```
d755  2 shields staff     4096 2026-05-14T18:23:11Z M  src
 644  1 shields staff     4567 2026-05-15T09:00:00Z  M README.md
l777  1 shields staff       45 2026-05-10T14:00:00Z ✓  bin → /usr/local/bin
 644  1 shields staff 12345678 2026-05-12T07:21:00Z !! node_modules.tar
 644  1 shields staff        0 2026-05-15T11:02:00Z ?? .DS_Store
```

Reading the git column: `src/` is staged-modified (`M ` = modified
in index, unchanged in worktree); `README.md` is dirty in worktree
but not staged (` M`); `bin` is tracked and clean (`✓ `);
`node_modules.tar` is gitignored (`!!`); `.DS_Store` is untracked
(`??`).
