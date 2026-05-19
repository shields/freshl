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

use std::cmp::Ordering;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;

use crate::case::Sensitivity;
use crate::entry::{Entry, EntryKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortKey {
    #[default]
    Name,
    Size,
    Time,
}

fn strip_leading_zeros(digits: &[u8]) -> &[u8] {
    let lead = digits.iter().take_while(|b| **b == b'0').count();
    &digits[lead..]
}

fn compare_digit_runs(a: &[u8], b: &[u8]) -> Ordering {
    let a_sig = strip_leading_zeros(a);
    let b_sig = strip_leading_zeros(b);
    a_sig.len().cmp(&b_sig.len()).then_with(|| a_sig.cmp(b_sig))
}

fn compare_within_group(a: &Entry, b: &Entry, sensitivity: Sensitivity, key: SortKey) -> Ordering {
    match key {
        SortKey::Name => natural_cmp(&a.name, &b.name, sensitivity),
        // Size and Time ascend by default (smallest / oldest first), so
        // the largest / newest entries land at the bottom of the output.
        // `-r` flips this for users who want ls-style descending order.
        // Tie-break with natural name order so equal values stay readable.
        SortKey::Size => a
            .size
            .cmp(&b.size)
            .then_with(|| natural_cmp(&a.name, &b.name, sensitivity)),
        SortKey::Time => a
            .mtime
            .cmp(&b.mtime)
            .then_with(|| natural_cmp(&a.name, &b.name, sensitivity)),
    }
}

#[must_use]
pub fn compare_by(a: &Entry, b: &Entry, sensitivity: Sensitivity, key: SortKey) -> Ordering {
    match (
        a.kind == EntryKind::Directory,
        b.kind == EntryKind::Directory,
    ) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => compare_within_group(a, b, sensitivity, key),
    }
}

#[must_use]
pub fn compare(a: &Entry, b: &Entry, sensitivity: Sensitivity) -> Ordering {
    compare_by(a, b, sensitivity, SortKey::Name)
}

pub fn sort_with(entries: &mut [Entry], sensitivity: Sensitivity, key: SortKey, reverse: bool) {
    // -r reverses the within-group order only; directories stay grouped at
    // the top regardless of the requested key.
    entries.sort_by(|a, b| {
        let a_dir = a.kind == EntryKind::Directory;
        let b_dir = b.kind == EntryKind::Directory;
        match (a_dir, b_dir) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => {
                let ord = compare_within_group(a, b, sensitivity, key);
                if reverse { ord.reverse() } else { ord }
            }
        }
    });
}

pub fn sort(entries: &mut [Entry], sensitivity: Sensitivity) {
    sort_with(entries, sensitivity, SortKey::Name, false);
}

#[must_use]
pub fn natural_cmp(a: &OsStr, b: &OsStr, sensitivity: Sensitivity) -> Ordering {
    let mut i = 0;
    let mut j = 0;
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    while i < ab.len() && j < bb.len() {
        let ca = ab[i];
        let cb = bb[j];
        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let ai = digit_run_end(ab, i);
            let bj = digit_run_end(bb, j);
            match compare_digit_runs(&ab[i..ai], &bb[j..bj]) {
                Ordering::Equal => {
                    i = ai;
                    j = bj;
                }
                other => return other,
            }
        } else {
            let (ka, kb) = match sensitivity {
                Sensitivity::Sensitive => (ca, cb),
                Sensitivity::Insensitive => (ca.to_ascii_lowercase(), cb.to_ascii_lowercase()),
            };
            match ka.cmp(&kb) {
                Ordering::Equal => {
                    i += 1;
                    j += 1;
                }
                other => return other,
            }
        }
    }
    // After the loop, at least one cursor has reached its slice's end. If
    // both have, fall back to lexicographic byte comparison so distinct names
    // that compare equal under the natural rule (e.g. `02a0` vs `2a00`) still
    // get a stable total order; otherwise the exhausted side is the shorter
    // (and therefore smaller) name.
    if i == ab.len() && j == bb.len() {
        ab.len().cmp(&bb.len()).then_with(|| ab.cmp(bb))
    } else if i == ab.len() {
        Ordering::Less
    } else {
        Ordering::Greater
    }
}

fn digit_run_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::{SortKey, compare, natural_cmp, sort, sort_with};
    use crate::case::Sensitivity;
    use crate::entry::{Entry, EntryKind};
    use std::cmp::Ordering;
    use std::ffi::{OsStr, OsString};
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    fn entry(name: &str, kind: EntryKind) -> Entry {
        Entry {
            name: OsString::from(name),
            path: PathBuf::from(name),
            kind,
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
        }
    }

    fn file_with_size(name: &str, size: u64) -> Entry {
        Entry {
            size,
            ..entry(name, EntryKind::RegularFile)
        }
    }

    fn file_with_mtime(name: &str, secs_after_epoch: u64) -> Entry {
        Entry {
            mtime: SystemTime::UNIX_EPOCH + Duration::from_secs(secs_after_epoch),
            ..entry(name, EntryKind::RegularFile)
        }
    }

    fn names(v: &[Entry]) -> Vec<String> {
        v.iter()
            .map(|e| e.name.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn natural_order_treats_digit_runs_as_numbers() {
        let cmp = |x, y| natural_cmp(OsStr::new(x), OsStr::new(y), Sensitivity::Sensitive);
        assert_eq!(cmp("f2", "f10"), Ordering::Less);
        assert_eq!(cmp("f10", "f2"), Ordering::Greater);
        assert_eq!(cmp("f02", "f2"), Ordering::Greater);
        assert_eq!(cmp("f2", "f02"), Ordering::Less);
        assert_eq!(cmp("a", "a"), Ordering::Equal);
        assert_eq!(cmp("a", "ab"), Ordering::Less);
        assert_eq!(cmp("ab", "a"), Ordering::Greater);
    }

    #[test]
    fn natural_order_compares_non_digit_chars() {
        let cmp = |x, y| natural_cmp(OsStr::new(x), OsStr::new(y), Sensitivity::Sensitive);
        assert_eq!(cmp("a", "b"), Ordering::Less);
        assert_eq!(cmp("z", "a"), Ordering::Greater);
        assert_eq!(cmp("a1", "b1"), Ordering::Less);
        assert_eq!(cmp("Ab", "ab"), Ordering::Less);
    }

    #[test]
    fn natural_order_insensitive_folds_ascii() {
        let cmp = |x, y| natural_cmp(OsStr::new(x), OsStr::new(y), Sensitivity::Insensitive);
        // Case-fold makes "Ab" and "ab" compare equal naturally, but the
        // raw-bytes tie-break breaks the tie deterministically so sorting
        // stays total.
        assert_eq!(cmp("Ab", "ab"), Ordering::Less);
        assert_eq!(cmp("AB", "ac"), Ordering::Less);
    }

    #[test]
    fn natural_order_mixed_letter_then_digit() {
        let cmp = |x, y| natural_cmp(OsStr::new(x), OsStr::new(y), Sensitivity::Sensitive);
        assert_eq!(cmp("a1", "ab"), Ordering::Less);
        assert_eq!(cmp("ab", "a1"), Ordering::Greater);
    }

    #[test]
    fn directories_sort_before_files() {
        let d = entry("z", EntryKind::Directory);
        let f = entry("a", EntryKind::RegularFile);
        assert_eq!(compare(&d, &f, Sensitivity::Sensitive), Ordering::Less);
        assert_eq!(compare(&f, &d, Sensitivity::Sensitive), Ordering::Greater);
    }

    #[test]
    fn symlink_to_file_sorts_with_files() {
        let link = entry("a_link", EntryKind::Symlink);
        let real = entry("z_dir", EntryKind::Directory);
        assert_eq!(
            compare(&real, &link, Sensitivity::Sensitive),
            Ordering::Less
        );
        assert_eq!(
            compare(&link, &real, Sensitivity::Sensitive),
            Ordering::Greater
        );
    }

    #[test]
    fn broken_symlink_sorts_with_files() {
        let broken = entry("a_broken", EntryKind::Symlink);
        let file = entry("b_file", EntryKind::RegularFile);
        let dir = entry("z_dir", EntryKind::Directory);
        assert_eq!(
            compare(&broken, &file, Sensitivity::Sensitive),
            Ordering::Less
        );
        assert_eq!(
            compare(&dir, &broken, Sensitivity::Sensitive),
            Ordering::Less
        );
    }

    #[test]
    fn within_a_group_natural_order_applies() {
        let a = entry("file2", EntryKind::RegularFile);
        let b = entry("file10", EntryKind::RegularFile);
        assert_eq!(compare(&a, &b, Sensitivity::Sensitive), Ordering::Less);

        let da = entry("Dir2", EntryKind::Directory);
        let db = entry("dir10", EntryKind::Directory);
        assert_eq!(compare(&da, &db, Sensitivity::Insensitive), Ordering::Less);
    }

    #[test]
    fn sort_orders_directories_first_then_natural() {
        let mut v = vec![
            entry("file2", EntryKind::RegularFile),
            entry("dir10", EntryKind::Directory),
            entry("file10", EntryKind::RegularFile),
            entry("dir2", EntryKind::Directory),
        ];
        sort(&mut v, Sensitivity::Sensitive);
        let names: Vec<_> = v
            .iter()
            .map(|e| e.name.to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["dir2", "dir10", "file2", "file10"]);
    }

    #[test]
    fn zero_padded_runs_compare_by_string_length_when_value_matches() {
        let cmp = |x, y| natural_cmp(OsStr::new(x), OsStr::new(y), Sensitivity::Sensitive);
        // Same numeric value (10) but different padding: the longer raw byte
        // form sorts later so the order is stable.
        assert_eq!(cmp("a010", "a0010"), Ordering::Less);
        assert_eq!(cmp("a0010", "a010"), Ordering::Greater);
    }

    #[test]
    fn unequal_tails_after_matching_digits_compare_by_remaining() {
        let cmp = |x, y| natural_cmp(OsStr::new(x), OsStr::new(y), Sensitivity::Sensitive);
        // Loop exits with one side's digit run exhausting the slice while the
        // other has non-digit bytes left; the side with leftover content is
        // longer and must sort after the exhausted one.
        assert_eq!(cmp("a0010", "a010a"), Ordering::Less);
        assert_eq!(cmp("a010a", "a0010"), Ordering::Greater);
    }

    #[test]
    fn enormous_digit_runs_do_not_panic() {
        let huge = "9".repeat(100);
        let other = "9".repeat(101);
        let result = natural_cmp(
            OsStr::new(&huge),
            OsStr::new(&other),
            Sensitivity::Sensitive,
        );
        assert_eq!(result, Ordering::Less);
    }

    #[test]
    fn size_key_sorts_files_smallest_first_with_name_tiebreak() {
        let mut v = vec![
            file_with_size("alpha", 100),
            file_with_size("zeta", 999),
            file_with_size("beta", 100),
        ];
        sort_with(&mut v, Sensitivity::Sensitive, SortKey::Size, false);
        assert_eq!(names(&v), vec!["alpha", "beta", "zeta"]);
    }

    #[test]
    fn time_key_sorts_files_oldest_first_with_name_tiebreak() {
        let mut v = vec![
            file_with_mtime("alpha", 100),
            file_with_mtime("zeta", 999),
            file_with_mtime("beta", 100),
        ];
        sort_with(&mut v, Sensitivity::Sensitive, SortKey::Time, false);
        assert_eq!(names(&v), vec!["alpha", "beta", "zeta"]);
    }

    #[test]
    fn directories_stay_grouped_first_under_size_key() {
        let mut v = vec![
            file_with_size("huge", 10_000_000),
            Entry {
                size: 4096,
                ..entry("tiny_dir", EntryKind::Directory)
            },
        ];
        sort_with(&mut v, Sensitivity::Sensitive, SortKey::Size, false);
        // Even though the file is larger, the directory still leads.
        assert_eq!(names(&v), vec!["tiny_dir", "huge"]);
    }

    #[test]
    fn directories_stay_grouped_first_under_time_key() {
        let mut v = vec![
            file_with_mtime("newer_file", 9_999),
            Entry {
                mtime: SystemTime::UNIX_EPOCH + Duration::from_secs(1),
                ..entry("older_dir", EntryKind::Directory)
            },
        ];
        sort_with(&mut v, Sensitivity::Sensitive, SortKey::Time, false);
        assert_eq!(names(&v), vec!["older_dir", "newer_file"]);
    }

    #[test]
    fn reverse_keeps_directories_first_but_reverses_within_groups() {
        let mut v = vec![
            entry("dir_a", EntryKind::Directory),
            entry("dir_b", EntryKind::Directory),
            entry("file_a", EntryKind::RegularFile),
            entry("file_b", EntryKind::RegularFile),
        ];
        sort_with(&mut v, Sensitivity::Sensitive, SortKey::Name, true);
        assert_eq!(names(&v), vec!["dir_b", "dir_a", "file_b", "file_a"]);
    }

    #[test]
    fn reverse_with_size_yields_descending_within_files() {
        let mut v = vec![
            file_with_size("small", 1),
            file_with_size("mid", 100),
            file_with_size("big", 9_999),
        ];
        sort_with(&mut v, Sensitivity::Sensitive, SortKey::Size, true);
        assert_eq!(names(&v), vec!["big", "mid", "small"]);
    }

    #[test]
    fn reverse_with_time_yields_newest_first_within_files() {
        let mut v = vec![
            file_with_mtime("old", 1),
            file_with_mtime("mid", 100),
            file_with_mtime("new", 9_999),
        ];
        sort_with(&mut v, Sensitivity::Sensitive, SortKey::Time, true);
        assert_eq!(names(&v), vec!["new", "mid", "old"]);
    }

    #[test]
    fn equal_size_files_break_tie_by_name_naturally() {
        let mut v = vec![
            file_with_size("file10", 42),
            file_with_size("file2", 42),
            file_with_size("file1", 42),
        ];
        sort_with(&mut v, Sensitivity::Sensitive, SortKey::Size, false);
        assert_eq!(names(&v), vec!["file1", "file2", "file10"]);
    }

    #[test]
    fn sort_wrapper_matches_sort_with_default_key() {
        let mut v1 = vec![
            entry("zeta", EntryKind::RegularFile),
            entry("alpha", EntryKind::RegularFile),
        ];
        let mut v2 = v1.clone();
        sort(&mut v1, Sensitivity::Sensitive);
        sort_with(&mut v2, Sensitivity::Sensitive, SortKey::Name, false);
        assert_eq!(names(&v1), names(&v2));
    }
}
