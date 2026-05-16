use std::collections::HashMap;
use std::ffi::OsString;

pub trait UserDirectory {
    fn user_name(&self, uid: u32) -> Option<OsString>;
    fn group_name(&self, gid: u32) -> Option<OsString>;
}

pub struct SystemDirectory;

impl UserDirectory for SystemDirectory {
    fn user_name(&self, uid: u32) -> Option<OsString> {
        uzers::get_user_by_uid(uid).map(|user| user.name().to_os_string())
    }

    fn group_name(&self, gid: u32) -> Option<OsString> {
        uzers::get_group_by_gid(gid).map(|group| group.name().to_os_string())
    }
}

pub struct OwnerCache<D: UserDirectory> {
    directory: D,
    users: HashMap<u32, OsString>,
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
        if let Some(name) = self.users.get(&uid) {
            return name.clone();
        }
        let name = self
            .directory
            .user_name(uid)
            .unwrap_or_else(|| OsString::from(uid.to_string()));
        self.users.insert(uid, name.clone());
        name
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
}

impl Default for OwnerCache<SystemDirectory> {
    fn default() -> Self {
        Self::new(SystemDirectory)
    }
}

#[cfg(test)]
mod tests {
    use super::{OwnerCache, SystemDirectory, UserDirectory};
    use std::cell::Cell;
    use std::ffi::OsString;

    struct Fixed {
        user: Option<&'static str>,
        group: Option<&'static str>,
        calls: Cell<usize>,
    }

    impl UserDirectory for Fixed {
        fn user_name(&self, _uid: u32) -> Option<OsString> {
            self.calls.set(self.calls.get() + 1);
            self.user.map(OsString::from)
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
            calls: Cell::new(0),
        });
        assert_eq!(cache.user(501), OsString::from("alice"));
        assert_eq!(cache.group(20), OsString::from("staff"));
    }

    #[test]
    fn unknown_uid_falls_back_to_numeric() {
        let mut cache = OwnerCache::new(Fixed {
            user: None,
            group: None,
            calls: Cell::new(0),
        });
        assert_eq!(cache.user(4242), OsString::from("4242"));
        assert_eq!(cache.group(99), OsString::from("99"));
    }

    #[test]
    fn lookups_are_cached() {
        let fixed = Fixed {
            user: Some("bob"),
            group: Some("admin"),
            calls: Cell::new(0),
        };
        let mut cache = OwnerCache::new(fixed);
        let _ = cache.user(7);
        let _ = cache.user(7);
        let _ = cache.group(7);
        let _ = cache.group(7);
        assert_eq!(cache.directory.calls.get(), 2);
    }

    #[test]
    fn default_constructor_uses_system_directory() {
        let _cache: OwnerCache<SystemDirectory> = OwnerCache::default();
    }

    #[test]
    fn system_directory_returns_some_or_none_without_panic() {
        let sys = SystemDirectory;
        let _ = sys.user_name(0);
        let _ = sys.group_name(0);
        let _ = sys.user_name(u32::MAX);
        let _ = sys.group_name(u32::MAX);
    }
}
