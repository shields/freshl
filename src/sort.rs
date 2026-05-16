use std::cmp::Ordering;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;

use crate::case::Sensitivity;
use crate::entry::{Entry, EntryKind};

fn strip_leading_zeros(digits: &[u8]) -> &[u8] {
    let lead = digits.iter().take_while(|b| **b == b'0').count();
    &digits[lead..]
}

fn compare_digit_runs(a: &[u8], b: &[u8]) -> Ordering {
    let a_sig = strip_leading_zeros(a);
    let b_sig = strip_leading_zeros(b);
    a_sig.len().cmp(&b_sig.len()).then_with(|| a_sig.cmp(b_sig))
}

#[must_use]
pub fn compare(a: &Entry, b: &Entry, sensitivity: Sensitivity) -> Ordering {
    match (a.kind, b.kind) {
        (EntryKind::Directory, EntryKind::Directory) => natural_cmp(&a.name, &b.name, sensitivity),
        (EntryKind::Directory, _) => Ordering::Less,
        (_, EntryKind::Directory) => Ordering::Greater,
        _ => natural_cmp(&a.name, &b.name, sensitivity),
    }
}

pub fn sort(entries: &mut [Entry], sensitivity: Sensitivity) {
    entries.sort_by(|a, b| compare(a, b, sensitivity));
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
    use super::{compare, natural_cmp, sort};
    use crate::case::Sensitivity;
    use crate::entry::{Entry, EntryKind};
    use std::cmp::Ordering;
    use std::ffi::{OsStr, OsString};
    use std::path::PathBuf;
    use std::time::SystemTime;

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
            mtime: SystemTime::UNIX_EPOCH,
            symlink_target: None,
        }
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
}
