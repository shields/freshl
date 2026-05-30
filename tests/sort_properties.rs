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

//! The sort comparator must be a strict total order — the contract
//! `slice::sort_by` relies on; a violating triple silently corrupts the listing.
//!
//! The comparator's behavior is fully determined by short inputs over a tiny
//! alphabet: every branch (digit runs, leading zeros, case-fold, the raw-bytes
//! length tiebreak) fires at length ≤4 over a handful of symbols. So instead of
//! *sampling* the space (fuzzing / random proptest), we enumerate it
//! **exhaustively** to a bound that exercises every branch — a proof over that
//! domain, deterministic and stronger. `sort_with`'s permutation/idempotence
//! stays property-based: the space of input *vectors* is unbounded.

// Bounded-exhaustive enumeration is supplementary to sort.rs's own unit tests
// (which provide the line coverage); excluded from the coverage build so the
// O(n²)/O(n³) sweeps don't slow the pre-commit hook.
#![cfg(not(coverage_nightly))]
#![expect(
    clippy::tests_outside_test_module,
    reason = "Cargo integration tests live at the file's module root"
)]

use std::cmp::Ordering;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};

use freshl::case::Sensitivity;
use freshl::entry::{Entry, EntryKind};
use freshl::sort::{SortKey, compare_by, natural_cmp, sort_with};

const SENSITIVITIES: [Sensitivity; 2] = [Sensitivity::Sensitive, Sensitivity::Insensitive];
const KEYS: [SortKey; 3] = [SortKey::Name, SortKey::Size, SortKey::Time];

/// Every byte string over `alphabet` of length `0..=max_len`.
fn enumerate(alphabet: &[u8], max_len: usize) -> Vec<OsString> {
    let mut all = vec![Vec::new()];
    let mut frontier = vec![Vec::<u8>::new()];
    for _ in 0..max_len {
        let mut next = Vec::with_capacity(frontier.len() * alphabet.len());
        for s in &frontier {
            for &b in alphabet {
                let mut t = s.clone();
                t.push(b);
                next.push(t);
            }
        }
        all.extend_from_slice(&next);
        frontier = next;
    }
    all.into_iter().map(OsString::from_vec).collect()
}

#[test]
fn natural_cmp_is_a_strict_total_order_exhaustive() {
    // Pairwise invariants over a broad bound — O(n²), so length 4 is cheap and
    // covers the tricky `02a0`/`2a00` natural-equal-but-distinct tiebreak.
    let pairs = enumerate(b"012aA.", 4); // 1555 strings
    for sens in SENSITIVITIES {
        for a in &pairs {
            assert_eq!(natural_cmp(a, a, sens), Ordering::Equal, "reflexivity");
            for b in &pairs {
                let ab = natural_cmp(a, b, sens);
                assert_eq!(ab, natural_cmp(b, a, sens).reverse(), "antisymmetry");
                // The raw-bytes tiebreak makes it strict: Equal iff byte-equal.
                assert_eq!(ab == Ordering::Equal, a == b, "Equal only for equal bytes");
            }
        }
    }

    // Transitivity is O(n³); a tighter bound still exercises every digit/letter
    // transition. Check only the ordered triples a ≤ b ≤ c.
    let tri = enumerate(b"01aA.", 3); // 156 strings
    for sens in SENSITIVITIES {
        for a in &tri {
            for b in &tri {
                if natural_cmp(a, b, sens) == Ordering::Greater {
                    continue;
                }
                for c in &tri {
                    if natural_cmp(b, c, sens) == Ordering::Greater {
                        continue;
                    }
                    assert_ne!(
                        natural_cmp(a, c, sens),
                        Ordering::Greater,
                        "transitivity: {a:?} <= {b:?} <= {c:?}"
                    );
                }
            }
        }
    }
}

fn mk_entry(name: &OsString, is_dir: bool, size: u64, mtime: u64) -> Entry {
    Entry {
        name: name.clone(),
        path: PathBuf::new(),
        kind: if is_dir {
            EntryKind::Directory
        } else {
            EntryKind::RegularFile
        },
        mode: 0,
        nlink: 0,
        uid: 0,
        gid: 0,
        size,
        rdev: 0,
        mtime: SystemTime::UNIX_EPOCH + Duration::from_secs(mtime),
        dev: 0,
        ino: 0,
        follow_chain: Vec::new(),
    }
}

#[test]
fn compare_by_is_a_total_order_for_every_key_exhaustive() {
    // A small but varied entry domain: names over {1,a} (len ≤2), both kinds,
    // and two distinct sizes/mtimes so the key ordering and the natural-name
    // tiebreak both engage. 7 names × 2 × 2 × 2 = 56 entries.
    let names = enumerate(b"1a", 2);
    let mut entries = Vec::new();
    for name in &names {
        for is_dir in [false, true] {
            for size in [0u64, 1] {
                for mtime in [0u64, 1] {
                    entries.push(mk_entry(name, is_dir, size, mtime));
                }
            }
        }
    }

    for key in KEYS {
        for sens in SENSITIVITIES {
            for a in &entries {
                assert_eq!(compare_by(a, a, sens, key), Ordering::Equal, "reflexivity");
                for b in &entries {
                    assert_eq!(
                        compare_by(a, b, sens, key),
                        compare_by(b, a, sens, key).reverse(),
                        "antisymmetry"
                    );
                }
            }
            for a in &entries {
                for b in &entries {
                    if compare_by(a, b, sens, key) == Ordering::Greater {
                        continue;
                    }
                    for c in &entries {
                        if compare_by(b, c, sens, key) == Ordering::Greater {
                            continue;
                        }
                        assert_ne!(
                            compare_by(a, c, sens, key),
                            Ordering::Greater,
                            "transitivity"
                        );
                    }
                }
            }
        }
    }
}

// `sort_with` over an *unbounded* space of input vectors: property-based, not
// enumerable. Asserts the two guarantees a correct total order yields.
fn name_strategy() -> impl Strategy<Value = OsString> {
    prop::collection::vec(prop::sample::select(b"01aA.".as_slice()), 0..6)
        .prop_map(OsString::from_vec)
}

fn entry_strategy() -> impl Strategy<Value = Entry> {
    (name_strategy(), any::<bool>(), 0u64..4, 0u64..4)
        .prop_map(|(name, is_dir, size, mtime)| mk_entry(&name, is_dir, size, mtime))
}

type Projected = (bool, Vec<u8>, u64, SystemTime);

fn project(e: &Entry) -> Projected {
    use std::os::unix::ffi::OsStrExt;
    (
        e.kind == EntryKind::Directory,
        e.name.as_bytes().to_vec(),
        e.size,
        e.mtime,
    )
}

#[test]
fn sort_with_is_a_permutation_and_idempotent() {
    let strat = (
        prop::collection::vec(entry_strategy(), 0..12),
        prop::sample::select(KEYS.as_slice()),
        prop::sample::select(SENSITIVITIES.as_slice()),
        any::<bool>(),
    );
    let mut config = Config::with_cases(512);
    config.failure_persistence = None;
    let mut runner =
        TestRunner::new_with_rng(config, TestRng::deterministic_rng(RngAlgorithm::ChaCha));
    runner
        .run(&strat, |(entries, key, sens, reverse)| {
            let mut once = entries.clone();
            sort_with(&mut once, sens, key, reverse);

            let mut before: Vec<Projected> = entries.iter().map(project).collect();
            let mut after: Vec<Projected> = once.iter().map(project).collect();
            before.sort();
            after.sort();
            prop_assert_eq!(&before, &after, "sort changed the multiset of entries");

            let mut twice = once.clone();
            sort_with(&mut twice, sens, key, reverse);
            let once_p: Vec<Projected> = once.iter().map(project).collect();
            let twice_p: Vec<Projected> = twice.iter().map(project).collect();
            prop_assert_eq!(once_p, twice_p, "sorting was not idempotent");
            Ok(())
        })
        .expect("sort_with property should hold");
}
