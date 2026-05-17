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

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sensitivity {
    Sensitive,
    Insensitive,
}

pub trait Detector {
    fn detect(&self, dir: &Path, samples: &[&OsStr]) -> Sensitivity;
}

#[cfg(target_os = "macos")]
#[must_use]
pub const fn platform_default() -> Sensitivity {
    Sensitivity::Insensitive
}

#[cfg(not(target_os = "macos"))]
#[must_use]
pub const fn platform_default() -> Sensitivity {
    Sensitivity::Sensitive
}

pub struct ProbeDetector;

impl Detector for ProbeDetector {
    fn detect(&self, dir: &Path, samples: &[&OsStr]) -> Sensitivity {
        for name in samples {
            if let Some(flipped) = flip_first_ascii_letter(name) {
                // If both the original and its case-flipped sibling are
                // present in this directory, the filesystem must be
                // case-sensitive — otherwise it couldn't host both names.
                // This avoids a hardlink-on-case-sensitive-FS misread where
                // `readme` and `Readme` share an inode and would otherwise
                // look like a case-fold to the inode probe.
                if samples.contains(&flipped.as_os_str()) {
                    return Sensitivity::Sensitive;
                }
                return probe_dir(dir, name, &flipped);
            }
        }
        platform_default()
    }
}

#[must_use]
pub fn flip_first_ascii_letter(name: &OsStr) -> Option<OsString> {
    let bytes = name.as_bytes();
    let pos = bytes.iter().position(u8::is_ascii_alphabetic)?;
    let mut out = bytes.to_vec();
    out[pos] = if out[pos].is_ascii_uppercase() {
        out[pos].to_ascii_lowercase()
    } else {
        out[pos].to_ascii_uppercase()
    };
    Some(OsString::from_vec(out))
}

fn probe_dir(dir: &Path, original: &OsStr, flipped: &OsStr) -> Sensitivity {
    classify_inos(stat_ino(&dir.join(original)), stat_ino(&dir.join(flipped)))
}

fn stat_ino(path: &Path) -> Option<u64> {
    std::fs::symlink_metadata(path).ok().map(|m| m.ino())
}

#[must_use]
pub const fn classify_inos(original: Option<u64>, flipped: Option<u64>) -> Sensitivity {
    match (original, flipped) {
        (Some(a), Some(b)) if a == b => Sensitivity::Insensitive,
        _ => Sensitivity::Sensitive,
    }
}

pub struct DetectorCache<D: Detector> {
    detector: D,
    map: HashMap<PathBuf, Sensitivity>,
}

impl<D: Detector> DetectorCache<D> {
    pub fn new(detector: D) -> Self {
        Self {
            detector,
            map: HashMap::new(),
        }
    }

    pub fn sensitivity(&mut self, path: &Path, samples: &[&OsStr]) -> Sensitivity {
        if let Some(s) = self.map.get(path) {
            return *s;
        }
        let s = self.detector.detect(path, samples);
        self.map.insert(path.to_path_buf(), s);
        s
    }
}

impl Default for DetectorCache<ProbeDetector> {
    fn default() -> Self {
        Self::new(ProbeDetector)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Detector, DetectorCache, ProbeDetector, Sensitivity, classify_inos,
        flip_first_ascii_letter, platform_default, probe_dir,
    };
    use std::cell::Cell;
    use std::ffi::{OsStr, OsString};
    use std::fs;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    struct Counting {
        result: Sensitivity,
        calls: Cell<usize>,
    }

    impl Detector for Counting {
        fn detect(&self, _dir: &Path, _samples: &[&OsStr]) -> Sensitivity {
            self.calls.set(self.calls.get() + 1);
            self.result
        }
    }

    #[test]
    fn probe_detector_treats_dir_with_both_cased_names_as_sensitive() {
        let dir = tempdir().unwrap();
        let s = ProbeDetector.detect(dir.path(), &[OsStr::new("readme"), OsStr::new("Readme")]);
        assert_eq!(s, Sensitivity::Sensitive);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn platform_default_is_insensitive_on_macos() {
        assert_eq!(platform_default(), Sensitivity::Insensitive);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn platform_default_is_sensitive_off_macos() {
        assert_eq!(platform_default(), Sensitivity::Sensitive);
    }

    #[test]
    fn flip_handles_first_lower_letter() {
        let result = flip_first_ascii_letter(OsStr::new("abc"));
        assert_eq!(result, Some(OsString::from("Abc")));
    }

    #[test]
    fn flip_handles_first_upper_letter() {
        let result = flip_first_ascii_letter(OsStr::new("ABC"));
        assert_eq!(result, Some(OsString::from("aBC")));
    }

    #[test]
    fn flip_skips_leading_non_letters() {
        let result = flip_first_ascii_letter(OsStr::new("123abc"));
        assert_eq!(result, Some(OsString::from("123Abc")));
    }

    #[test]
    fn flip_returns_none_when_no_letter() {
        assert_eq!(flip_first_ascii_letter(OsStr::new("12345")), None);
        assert_eq!(flip_first_ascii_letter(OsStr::new("")), None);
    }

    #[test]
    fn classify_matching_inos_is_insensitive() {
        assert_eq!(classify_inos(Some(42), Some(42)), Sensitivity::Insensitive);
    }

    #[test]
    fn classify_differing_inos_is_sensitive() {
        assert_eq!(classify_inos(Some(1), Some(2)), Sensitivity::Sensitive);
    }

    #[test]
    fn classify_missing_flipped_is_sensitive() {
        assert_eq!(classify_inos(Some(1), None), Sensitivity::Sensitive);
    }

    #[test]
    fn classify_missing_both_is_sensitive() {
        assert_eq!(classify_inos(None, None), Sensitivity::Sensitive);
    }

    #[test]
    fn probe_dir_agrees_with_direct_stat_calls() {
        // probe_dir's job is to wire stat→classify_inos for two names. Verify
        // that wiring against a real filesystem without asserting which mode
        // the filesystem is in (which varies between case-sensitive ext4 on
        // CI Linux and case-insensitive APFS on macOS dev machines).
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("readme"), b"x").unwrap();
        let observed = probe_dir(dir.path(), OsStr::new("readme"), OsStr::new("README"));
        let expected = classify_inos(
            fs::symlink_metadata(dir.path().join("readme"))
                .ok()
                .map(|m| m.ino()),
            fs::symlink_metadata(dir.path().join("README"))
                .ok()
                .map(|m| m.ino()),
        );
        assert_eq!(observed, expected);
    }

    #[test]
    fn probe_detector_falls_back_when_no_letter_sample() {
        let dir = tempdir().unwrap();
        let s = ProbeDetector.detect(dir.path(), &[OsStr::new("123")]);
        assert_eq!(s, platform_default());
    }

    #[test]
    fn probe_detector_falls_back_on_empty_samples() {
        let dir = tempdir().unwrap();
        let s = ProbeDetector.detect(dir.path(), &[]);
        assert_eq!(s, platform_default());
    }

    #[test]
    fn probe_detector_skips_non_letter_then_uses_letter() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("hello"), b"x").unwrap();
        let _ = ProbeDetector.detect(dir.path(), &[OsStr::new("123"), OsStr::new("hello")]);
    }

    #[test]
    fn cache_returns_detector_result() {
        let mut cache = DetectorCache::new(Counting {
            result: Sensitivity::Insensitive,
            calls: Cell::new(0),
        });
        assert_eq!(
            cache.sensitivity(&PathBuf::from("/x"), &[]),
            Sensitivity::Insensitive
        );
    }

    #[test]
    fn cache_avoids_repeat_calls() {
        let counting = Counting {
            result: Sensitivity::Sensitive,
            calls: Cell::new(0),
        };
        let mut cache = DetectorCache::new(counting);
        let _ = cache.sensitivity(Path::new("/x"), &[]);
        let _ = cache.sensitivity(Path::new("/x"), &[]);
        let _ = cache.sensitivity(Path::new("/y"), &[]);
        assert_eq!(cache.detector.calls.get(), 2);
    }

    #[test]
    fn default_constructor_uses_probe_detector() {
        let mut cache: DetectorCache<ProbeDetector> = DetectorCache::default();
        let dir = tempdir().unwrap();
        let _ = cache.sensitivity(dir.path(), &[]);
    }
}
