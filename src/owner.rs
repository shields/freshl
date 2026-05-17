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
use std::ffi::OsString;

/// One passwd entry. Bundled so `user_name` and `primary_gid` lookups share
/// a single `getpwuid` call per uid instead of paying it twice.
pub struct UserRecord {
    pub name: OsString,
    pub primary_gid: u32,
}

pub trait UserDirectory {
    fn lookup_user(&self, uid: u32) -> Option<UserRecord>;
    fn group_name(&self, gid: u32) -> Option<OsString>;
}

pub struct SystemDirectory;

impl UserDirectory for SystemDirectory {
    fn lookup_user(&self, uid: u32) -> Option<UserRecord> {
        uzers::get_user_by_uid(uid).map(|user| UserRecord {
            name: user.name().to_os_string(),
            primary_gid: user.primary_group_id(),
        })
    }

    fn group_name(&self, gid: u32) -> Option<OsString> {
        uzers::get_group_by_gid(gid).map(|group| group.name().to_os_string())
    }
}

pub struct OwnerCache<D: UserDirectory> {
    directory: D,
    users: HashMap<u32, Option<UserRecord>>,
    groups: HashMap<u32, OsString>,
}

impl<D: UserDirectory> OwnerCache<D> {
    pub fn new(directory: D) -> Self {
        Self {
            directory,
            users: HashMap::new(),
            groups: HashMap::new(),
        }
    }

    pub fn user(&mut self, uid: u32) -> OsString {
        self.user_record(uid)
            .map_or_else(|| OsString::from(uid.to_string()), |r| r.name.clone())
    }

    pub fn group(&mut self, gid: u32) -> OsString {
        if let Some(name) = self.groups.get(&gid) {
            return name.clone();
        }
        let name = self
            .directory
            .group_name(gid)
            .unwrap_or_else(|| OsString::from(gid.to_string()));
        self.groups.insert(gid, name.clone());
        name
    }

    pub fn primary_gid(&mut self, uid: u32) -> Option<u32> {
        self.user_record(uid).map(|r| r.primary_gid)
    }

    pub fn gid_is_primary(&mut self, uid: u32, gid: u32) -> bool {
        self.primary_gid(uid) == Some(gid)
    }

    fn user_record(&mut self, uid: u32) -> Option<&UserRecord> {
        let Self {
            users, directory, ..
        } = self;
        users
            .entry(uid)
            .or_insert_with(|| directory.lookup_user(uid))
            .as_ref()
    }
}

impl Default for OwnerCache<SystemDirectory> {
    fn default() -> Self {
        Self::new(SystemDirectory)
    }
}

#[cfg(test)]
mod tests {
    use super::{OwnerCache, SystemDirectory, UserDirectory, UserRecord};
    use std::cell::Cell;
    use std::ffi::OsString;

    struct Fixed {
        user: Option<&'static str>,
        group: Option<&'static str>,
        primary: u32,
        calls: Cell<usize>,
    }

    impl UserDirectory for Fixed {
        fn lookup_user(&self, _uid: u32) -> Option<UserRecord> {
            self.calls.set(self.calls.get() + 1);
            self.user.map(|name| UserRecord {
                name: OsString::from(name),
                primary_gid: self.primary,
            })
        }
        fn group_name(&self, _gid: u32) -> Option<OsString> {
            self.calls.set(self.calls.get() + 1);
            self.group.map(OsString::from)
        }
    }

    #[test]
    fn user_lookup_returns_name_when_present() {
        let mut cache = OwnerCache::new(Fixed {
            user: Some("alice"),
            group: Some("staff"),
            primary: 20,
            calls: Cell::new(0),
        });
        assert_eq!(cache.user(501), OsString::from("alice"));
        assert_eq!(cache.group(20), OsString::from("staff"));
        assert_eq!(cache.primary_gid(501), Some(20));
    }

    #[test]
    fn unknown_uid_falls_back_to_numeric() {
        let mut cache = OwnerCache::new(Fixed {
            user: None,
            group: None,
            primary: 0,
            calls: Cell::new(0),
        });
        assert_eq!(cache.user(4242), OsString::from("4242"));
        assert_eq!(cache.group(99), OsString::from("99"));
        assert_eq!(cache.primary_gid(4242), None);
    }

    #[test]
    fn user_and_primary_gid_share_one_passwd_lookup() {
        let fixed = Fixed {
            user: Some("bob"),
            group: Some("admin"),
            primary: 7,
            calls: Cell::new(0),
        };
        let mut cache = OwnerCache::new(fixed);
        let _ = cache.user(7);
        let _ = cache.user(7);
        let _ = cache.group(7);
        let _ = cache.group(7);
        let _ = cache.primary_gid(7);
        let _ = cache.primary_gid(7);
        // 1 lookup_user + 1 group_name = 2 calls total; the second user(), group(),
        // and both primary_gid() calls all hit the cache.
        assert_eq!(cache.directory.calls.get(), 2);
    }

    #[test]
    fn gid_is_primary_matches_passwd_entry() {
        let mut cache = OwnerCache::new(Fixed {
            user: Some("alice"),
            group: Some("staff"),
            primary: 20,
            calls: Cell::new(0),
        });
        assert!(cache.gid_is_primary(501, 20));
        assert!(!cache.gid_is_primary(501, 30));
    }

    #[test]
    fn gid_is_primary_false_for_unknown_user() {
        let mut cache = OwnerCache::new(Fixed {
            user: None,
            group: None,
            primary: 0,
            calls: Cell::new(0),
        });
        assert!(!cache.gid_is_primary(4242, 20));
    }

    #[test]
    fn default_constructor_uses_system_directory() {
        let _cache: OwnerCache<SystemDirectory> = OwnerCache::default();
    }

    #[test]
    fn system_directory_returns_some_or_none_without_panic() {
        let sys = SystemDirectory;
        let _ = sys.lookup_user(0);
        let _ = sys.group_name(0);
        let _ = sys.lookup_user(u32::MAX);
        let _ = sys.group_name(u32::MAX);
    }
}
