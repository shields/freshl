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

//! Whole-program property suite over a generated filesystem tree.
//!
//! The generator *is* the oracle: it knows every entry's raw name bytes, so it
//! can assert freshl preserves them. The other oracles are structural —
//! freshl must never panic, must be deterministic, and must produce
//! column-aligned, well-formed output — properties that hold for *any* valid
//! tree, which is exactly what a generator can search and hand-written examples
//! cannot. Names are drawn from arbitrary ASCII (control characters, newlines,
//! ESC, DEL) to stress the byte-faithful rendering path. Names are capped at
//! ASCII, not arbitrary bytes, because macOS APFS rejects non-UTF-8 filenames
//! (EILSEQ) and NFD-normalizes Unicode — neither would survive a round trip
//! through the filesystem. Non-UTF-8 byte fidelity is a unit test
//! (`format::name::non_utf8_name_round_trips_exactly`).
//!
//! Special files (FIFO/socket/device) and unreadable permissions are covered by
//! the deterministic gap tests in tests/integration.rs, not here; this suite
//! sticks to files, directories, and symlinks (incl. broken and parent-pointing
//! cycle links), where every entry materializes from std alone.

// Heavy (materializes a tree + runs freshl per case): excluded from the
// coverage *build* to keep the pre-commit hook fast. Runs under `make test`.
#![cfg(not(coverage_nightly))]
#![expect(
    clippy::tests_outside_test_module,
    reason = "Cargo integration tests live at the file's module root"
)]
#![expect(
    clippy::unwrap_used,
    reason = "a generator/materialization failure should abort the test loudly"
)]

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::process::ExitCode;

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
use tempfile::TempDir;

fn check<S>(strategy: S, test: impl Fn(S::Value) -> Result<(), TestCaseError>)
where
    S: Strategy,
    S::Value: std::fmt::Debug,
{
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(128);
    let mut config = Config::with_cases(cases);
    config.failure_persistence = None;
    let mut runner =
        TestRunner::new_with_rng(config, TestRng::deterministic_rng(RngAlgorithm::ChaCha));
    runner
        .run(&strategy, test)
        .expect("property should hold for every generated tree");
}

// ---------------------------------------------------------------------------
// Tree generation
// ---------------------------------------------------------------------------

/// What to materialize at a leaf. A symlink carries its raw target bytes; most
/// dangle, some land on a sibling (see `node_strategy`).
#[derive(Clone, Debug)]
enum Node {
    File,
    Dir,
    Symlink(Vec<u8>),
}

/// Any ASCII byte is a legal, round-trippable filename byte except `/` and NUL.
/// Includes the control range (newline, tab, ESC, DEL) to exercise byte-faithful
/// rendering without tripping APFS's UTF-8 enforcement.
fn wild_byte() -> impl Strategy<Value = u8> {
    (1u8..=0x7f).prop_filter("not /", |b| *b != b'/')
}

fn wild_bytes(max: usize) -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(wild_byte(), 0..max)
}

fn node_strategy() -> impl Strategy<Value = Node> {
    // Symlink targets are single in-tree components (no `/`, never `.`/`..`), so
    // a link can dangle or point at a sibling but never escape the tempdir.
    // A `..` target would make `-R` descend into the shared, concurrently
    // mutating system temp directory and break determinism. (Symlink cycles
    // have their own unit test in lib.rs.)
    let symlink = prop::collection::vec(wild_byte(), 1..6)
        .prop_filter("not . or ..", |t| {
            t.as_slice() != b"." && t.as_slice() != b".."
        })
        .prop_map(Node::Symlink);
    prop_oneof![
        3 => Just(Node::File),
        2 => Just(Node::Dir),
        2 => symlink,
    ]
}

/// One entry: a parent path drawn from a tiny directory pool (`g0`/`g1`), the
/// leaf's arbitrary name bytes, and what to put there.
type EntrySpec = (Vec<u8>, Vec<u8>, Node);

fn entry_strategy() -> impl Strategy<Value = EntrySpec> {
    (
        prop::collection::vec(0u8..2, 0..=2),
        wild_bytes(6),
        node_strategy(),
    )
}

/// The unique on-disk leaf name: an index prefix guarantees uniqueness and
/// rules out the illegal `.`/`..` names, while the arbitrary suffix keeps the
/// trailing bytes adversarial.
fn leaf_name(idx: usize, raw: &[u8]) -> Vec<u8> {
    let mut name = format!("{idx}_").into_bytes();
    name.extend_from_slice(raw);
    name
}

fn materialize(entries: &[EntrySpec]) -> TempDir {
    let tmp = tempfile::tempdir().unwrap();
    for (i, (dirs, raw, node)) in entries.iter().enumerate() {
        let mut parent = tmp.path().to_path_buf();
        for d in dirs {
            parent.push(format!("g{d}"));
        }
        std::fs::create_dir_all(&parent).unwrap();
        let path = parent.join(OsStr::from_bytes(&leaf_name(i, raw)));
        match node {
            Node::File => {
                std::fs::write(&path, b"x").unwrap();
            }
            Node::Dir => {
                std::fs::create_dir(&path).unwrap();
            }
            Node::Symlink(target) => {
                std::os::unix::fs::symlink(OsStr::from_bytes(target), &path).unwrap();
            }
        }
    }
    tmp
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn code_repr(code: ExitCode) -> String {
    format!("{code:?}")
}

fn run(args: &[&OsStr]) -> (ExitCode, Vec<u8>, Vec<u8>) {
    let owned: Vec<OsString> = args.iter().map(|a| a.to_os_string()).collect();
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = freshl::run(owned, &mut out, &mut err);
    (code, out, err)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

/// Drop `ESC [ … m` SGR sequences. Safe only when names hold no raw ESC, which
/// the tame generator guarantees.
fn strip_ansi(line: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(line.len());
    let mut it = line.iter().copied();
    while let Some(b) = it.next() {
        if b == 0x1b {
            for c in it.by_ref() {
                if c == b'm' {
                    break;
                }
            }
        } else {
            out.push(b);
        }
    }
    out
}

/// Whether `w` opens with `YYYY-MM-DDTHH:MM:SSZ`.
fn is_iso(w: &[u8]) -> bool {
    let d = |i: usize| w.get(i).is_some_and(u8::is_ascii_digit);
    let lit = |i: usize, c: u8| w.get(i) == Some(&c);
    d(0) && d(1)
        && d(2)
        && d(3)
        && lit(4, b'-')
        && d(5)
        && d(6)
        && lit(7, b'-')
        && d(8)
        && d(9)
        && lit(10, b'T')
        && d(11)
        && d(12)
        && lit(13, b':')
        && d(14)
        && d(15)
        && lit(16, b':')
        && d(17)
        && d(18)
        && lit(19, b'Z')
}

/// Byte offset of the mtime column in a row (its first `…Z` ISO timestamp).
fn iso_offset(stripped: &[u8]) -> Option<usize> {
    stripped.windows(20).position(is_iso)
}

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

#[test]
fn recursive_listing_is_robust_deterministic_and_byte_faithful() {
    check(prop::collection::vec(entry_strategy(), 0..=8), |entries| {
        let tmp = materialize(&entries);
        let root = tmp.path().as_os_str();
        let dash_r = OsStr::new("-R");

        let (code, out, err) = run(&[dash_r, root]);
        // A readable tree (no permission tricks) always lists cleanly.
        prop_assert_eq!(
            code_repr(code),
            code_repr(ExitCode::SUCCESS),
            "non-success exit; stderr: {}",
            String::from_utf8_lossy(&err)
        );

        // Determinism: a pure function of the filesystem snapshot.
        let (_, rerun, _) = run(&[dash_r, root]);
        prop_assert!(out == rerun, "output was not deterministic");

        // Byte-faithfulness: every entry's raw name survives to the output.
        for (i, (_, raw, _)) in entries.iter().enumerate() {
            let name = leaf_name(i, raw);
            prop_assert!(
                contains(&out, &name),
                "name {:?} missing from output",
                String::from_utf8_lossy(&name)
            );
        }
        Ok(())
    });
}

/// Printable bytes only (no `/`, no control chars), so ANSI stripping is exact
/// and each entry occupies exactly one line.
const TAME: &[u8] = b"abAB019.- _zZ";

fn tame_entry() -> impl Strategy<Value = (Vec<u8>, Node)> {
    let name = prop::collection::vec(prop::sample::select(TAME), 0..6);
    let node = prop_oneof![
        Just(Node::File),
        Just(Node::Dir),
        prop::collection::vec(prop::sample::select(TAME), 1..5).prop_map(Node::Symlink),
    ];
    (name, node)
}

#[test]
fn columns_are_aligned_across_a_block() {
    check(prop::collection::vec(tame_entry(), 0..=8), |entries| {
        // Flat tree at the root: a non-recursive listing renders it as one
        // block (no label line), one row per entry.
        let flat: Vec<EntrySpec> = entries
            .into_iter()
            .map(|(name, node)| (Vec::new(), name, node))
            .collect();
        let tmp = materialize(&flat);
        let (code, out, _) = run(&[tmp.path().as_os_str()]);
        prop_assert_eq!(code_repr(code), code_repr(ExitCode::SUCCESS));

        let lines: Vec<&[u8]> = out
            .split(|&b| b == b'\n')
            .filter(|l| !l.is_empty())
            .collect();
        prop_assert_eq!(lines.len(), flat.len(), "one row per entry expected");

        let mut offsets = Vec::new();
        for line in &lines {
            let stripped = strip_ansi(line);
            let off = iso_offset(&stripped);
            prop_assert!(off.is_some(), "row has no mtime column: {:?}", stripped);
            offsets.push(off);
        }
        if let Some(first) = offsets.first() {
            prop_assert!(
                offsets.iter().all(|o| o == first),
                "mtime columns not aligned: {offsets:?}"
            );
        }
        Ok(())
    });
}
