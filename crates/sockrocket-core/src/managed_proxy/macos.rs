//! Exact ownership of proxy fields, keyed by stable network-service ID.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Service {
    pub id: String,
    #[serde(default = "existing")]
    pub before_exists: bool,
    pub before: BTreeMap<String, Option<String>>,
    pub applied: BTreeMap<String, Option<String>>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct Snapshot {
    pub version: u32,
    pub services: Vec<Service>,
}
fn existing() -> bool {
    true
}
#[cfg(any(target_os = "macos", test))]
const GROUPS: &[&[&str]] = &[
    &["HTTPEnable", "HTTPProxy", "HTTPPort"],
    &["HTTPSEnable", "HTTPSProxy", "HTTPSPort"],
    &["SOCKSEnable", "SOCKSProxy", "SOCKSPort"],
    &["ExceptionsList"],
];
#[cfg(any(target_os = "macos", test))]
fn restoration(
    current: &BTreeMap<String, Option<String>>,
    owned: &Service,
) -> BTreeMap<String, Option<String>> {
    let mut result = current.clone();
    for group in GROUPS {
        if group
            .iter()
            .all(|key| current.get(*key) == owned.applied.get(*key))
        {
            for key in *group {
                result.insert((*key).into(), owned.before.get(*key).cloned().flatten());
            }
        }
    }
    result
}
#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use anyhow::{Context, Result, ensure};
    use core_foundation::{
        array::CFArray,
        base::{CFRelease, CFType, TCFType},
        data::CFData,
        dictionary::{CFDictionary, CFMutableDictionary},
        number::CFNumber,
        propertylist,
        string::CFString,
    };
    use std::{
        ffi::{c_char, c_void},
        ptr,
        sync::{Mutex, OnceLock},
    };
    type Raw = *const c_void;
    type Dict = CFDictionary<CFString, CFType>;
    #[link(name = "SystemConfiguration", kind = "framework")]
    unsafe extern "C" {
        fn SCPreferencesCreate(a: Raw, n: Raw, p: Raw) -> Raw;
        fn SCPreferencesCreateWithAuthorization(a: Raw, n: Raw, p: Raw, auth: Raw) -> Raw;
        fn SCPreferencesPathGetValue(p: Raw, path: Raw) -> Raw;
        fn SCPreferencesPathSetValue(p: Raw, path: Raw, v: Raw) -> u8;
        fn SCPreferencesPathRemoveValue(p: Raw, path: Raw) -> u8;
        fn SCPreferencesLock(p: Raw, wait: u8) -> u8;
        fn SCPreferencesUnlock(p: Raw) -> u8;
        fn SCPreferencesCommitChanges(p: Raw) -> u8;
        fn SCPreferencesApplyChanges(p: Raw) -> u8;
        fn SCError() -> i32;
    }
    #[repr(C)]
    struct Item {
        name: *const c_char,
        len: usize,
        value: *mut c_void,
        flags: u32,
    }
    #[repr(C)]
    struct Rights {
        count: u32,
        items: *mut Item,
    }
    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        fn AuthorizationCreate(
            rights: *const Rights,
            env: Raw,
            flags: u32,
            result: *mut Raw,
        ) -> i32;
        fn AuthorizationCopyRights(
            auth: Raw,
            rights: *const Rights,
            env: Raw,
            flags: u32,
            result: Raw,
        ) -> i32;
        fn AuthorizationFree(auth: Raw, flags: u32) -> i32;
        fn AuthorizationMakeExternalForm(auth: Raw, result: *mut [u8; 32]) -> i32;
        fn AuthorizationCreateFromExternalForm(form: *const [u8; 32], auth: *mut Raw) -> i32;
    }
    struct Auth(usize);
    impl Drop for Auth {
        fn drop(&mut self) {
            unsafe {
                AuthorizationFree(self.0 as Raw, 0);
            }
        }
    }
    fn auth() -> &'static Mutex<Option<Auth>> {
        static A: OnceLock<Mutex<Option<Auth>>> = OnceLock::new();
        A.get_or_init(|| Mutex::new(None))
    }
    pub(crate) fn import(form: [u8; 32]) -> Result<()> {
        let mut raw = ptr::null();
        ensure!(
            unsafe { AuthorizationCreateFromExternalForm(&form, &mut raw) } == 0,
            "Recovery authorization unavailable"
        );
        *auth().lock().unwrap() = Some(Auth(raw as usize));
        Ok(())
    }
    fn authorization(interactive: bool) -> Result<Raw> {
        if unsafe { libc::geteuid() } == 0 {
            return Ok(ptr::null());
        }
        let mut guard = auth().lock().unwrap();
        if guard.is_none() {
            let mut raw = ptr::null();
            ensure!(
                unsafe { AuthorizationCreate(ptr::null(), ptr::null(), 0, &mut raw) } == 0,
                "Cannot create network authorization"
            );
            *guard = Some(Auth(raw as usize));
        }
        let raw = guard.as_ref().unwrap().0 as Raw;
        let mut item = Item {
            name: c"system.preferences.network".as_ptr(),
            len: 0,
            value: ptr::null_mut(),
            flags: 0,
        };
        let rights = Rights {
            count: 1,
            items: &mut item,
        };
        ensure!(
            unsafe {
                AuthorizationCopyRights(
                    raw,
                    &rights,
                    ptr::null(),
                    2 | if interactive { 1 } else { 0 },
                    ptr::null(),
                )
            } == 0,
            "Network preferences authorization was denied or expired"
        );
        if interactive {
            let mut form = [0u8; 32];
            ensure!(
                unsafe { AuthorizationMakeExternalForm(raw, &mut form) } == 0,
                "Cannot share recovery authorization"
            );
            crate::network_guardian::share_macos_authorization(form)?;
        }
        Ok(raw)
    }
    struct Prefs(Raw, bool);
    impl Drop for Prefs {
        fn drop(&mut self) {
            unsafe {
                if self.1 {
                    SCPreferencesUnlock(self.0);
                }
                CFRelease(self.0);
            }
        }
    }
    impl Prefs {
        fn open(write: bool, interactive: bool) -> Result<Self> {
            let name = CFString::new("Sockrocket network ownership");
            let auth = if write {
                authorization(interactive)?
            } else {
                ptr::null()
            };
            let raw = unsafe {
                if auth.is_null() {
                    SCPreferencesCreate(ptr::null(), name.as_CFTypeRef(), ptr::null())
                } else {
                    SCPreferencesCreateWithAuthorization(
                        ptr::null(),
                        name.as_CFTypeRef(),
                        ptr::null(),
                        auth,
                    )
                }
            };
            ensure!(!raw.is_null(), "Cannot open network preferences");
            // Fresh read sessions need no exclusive write lock or authorization.
            // Locking an unauthorized verification session can fail even after
            // a successful commit and incorrectly trigger proxy rollback.
            if write && unsafe { SCPreferencesLock(raw, 0) } == 0 {
                let code = unsafe { SCError() };
                unsafe { CFRelease(raw) };
                anyhow::bail!("Cannot lock network preferences (SystemConfiguration error {code})");
            }
            Ok(Self(raw, write))
        }
        fn get(&self, path: &str) -> Option<Dict> {
            let p = CFString::new(path);
            let raw = unsafe { SCPreferencesPathGetValue(self.0, p.as_CFTypeRef()) };
            if raw.is_null() {
                None
            } else {
                as_dict(unsafe { CFType::wrap_under_get_rule(raw) })
            }
        }
        fn set(&self, path: &str, value: &Dict) -> Result<()> {
            let path = CFString::new(path);
            ensure!(
                unsafe {
                    SCPreferencesPathSetValue(self.0, path.as_CFTypeRef(), value.as_CFTypeRef())
                } != 0,
                "Cannot stage network proxy preferences"
            );
            Ok(())
        }
        fn remove(&self, path: &str) -> Result<()> {
            let path = CFString::new(path);
            ensure!(
                unsafe { SCPreferencesPathRemoveValue(self.0, path.as_CFTypeRef()) } != 0,
                "Cannot remove owned proxy preferences"
            );
            Ok(())
        }
        fn commit(&self) -> Result<()> {
            ensure!(
                unsafe { SCPreferencesCommitChanges(self.0) } != 0,
                "Cannot commit network proxy preferences"
            );
            ensure!(
                unsafe { SCPreferencesApplyChanges(self.0) } != 0,
                "Cannot apply network proxy preferences"
            );
            Ok(())
        }
    }
    fn as_dict(value: CFType) -> Option<Dict> {
        let untyped = value.downcast::<CFDictionary>()?;
        Some(unsafe { Dict::wrap_under_get_rule(untyped.as_concrete_TypeRef()) })
    }
    fn field(d: &Dict, key: &str) -> Option<CFType> {
        d.find(CFString::new(key)).map(|v| (*v).clone())
    }
    fn encode(v: &CFType) -> Result<String> {
        let data = propertylist::create_data(
            v.as_CFTypeRef(),
            propertylist::kCFPropertyListXMLFormat_v1_0,
        )
        .map_err(|_| anyhow::anyhow!("Cannot serialize proxy setting"))?;
        Ok(String::from_utf8(data.bytes().to_vec())?)
    }
    fn decode(s: &str) -> Result<CFType> {
        let (raw, _) = propertylist::create_with_data(CFData::from_buffer(s.as_bytes()), 0)
            .map_err(|_| anyhow::anyhow!("Invalid saved proxy setting"))?;
        Ok(unsafe { CFType::wrap_under_create_rule(raw) })
    }
    fn capture(d: &Dict) -> Result<BTreeMap<String, Option<String>>> {
        GROUPS
            .iter()
            .flat_map(|group| group.iter())
            .map(|key| {
                Ok((
                    (*key).to_string(),
                    field(d, key).as_ref().map(encode).transpose()?,
                ))
            })
            .collect()
    }
    fn overlay(current: &Dict, values: &BTreeMap<String, Option<String>>) -> Result<Dict> {
        let (keys, vals) = current.get_keys_and_values();
        let mut pairs = Vec::new();
        for (k, v) in keys.into_iter().zip(vals) {
            pairs.push(
                (unsafe { CFString::wrap_under_get_rule(k.cast()) }, unsafe {
                    CFType::wrap_under_get_rule(v)
                }),
            );
        }
        let mut result = CFMutableDictionary::from_CFType_pairs(&pairs);
        for (k, v) in values {
            let key = CFString::new(k);
            if let Some(v) = v {
                result.set(key, decode(v)?);
            } else {
                result.remove(key);
            }
        }
        Ok(unsafe { Dict::wrap_under_get_rule(result.as_CFTypeRef().cast()) })
    }
    fn empty() -> Dict {
        Dict::from_CFType_pairs(&[])
    }
    fn path(id: &str) -> String {
        format!("/NetworkServices/{id}/Proxies")
    }
    fn desired(host: &str, port: u16) -> Result<BTreeMap<String, Option<String>>> {
        let mut d = BTreeMap::new();
        for prefix in ["HTTP", "HTTPS"] {
            d.insert(
                format!("{prefix}Enable"),
                Some(encode(&CFNumber::from(1i32).as_CFType())?),
            );
            d.insert(
                format!("{prefix}Proxy"),
                Some(encode(&CFString::new(host).as_CFType())?),
            );
            d.insert(
                format!("{prefix}Port"),
                Some(encode(&CFNumber::from(port as i32).as_CFType())?),
            );
        }
        // This API receives the HTTP listener port, which cannot speak SOCKS.
        // Disable SOCKS while owning the proxy; the journal restores its exact
        // previous values on exit.
        d.insert(
            "SOCKSEnable".into(),
            Some(encode(&CFNumber::from(0i32).as_CFType())?),
        );
        d.insert("SOCKSProxy".into(), None);
        d.insert("SOCKSPort".into(), None);
        let bypass = CFArray::from_CFTypes(&[
            CFString::new("localhost"),
            CFString::new("127.0.0.1"),
            CFString::new("::1"),
        ]);
        d.insert("ExceptionsList".into(), Some(encode(&bypass.as_CFType())?));
        Ok(d)
    }
    #[cfg(test)]
    mod native_tests {
        use super::*;

        #[test]
        fn verification_reads_without_an_authorized_write_lock() {
            let prefs = Prefs::open(false, false).unwrap();
            assert!(!prefs.1);
            assert!(prefs.get("/NetworkServices").is_some());
        }

        #[test]
        fn http_listener_is_not_advertised_as_a_socks_proxy() {
            let fields = desired("127.0.0.1", 1088).unwrap();
            for key in ["HTTPEnable", "HTTPSEnable"] {
                let value = decode(fields[key].as_ref().unwrap()).unwrap();
                assert_eq!(value.downcast::<CFNumber>().unwrap().to_i32(), Some(1));
            }
            let enabled = decode(fields["SOCKSEnable"].as_ref().unwrap()).unwrap();
            assert_eq!(enabled.downcast::<CFNumber>().unwrap().to_i32(), Some(0));
            assert_eq!(fields["SOCKSProxy"], None);
            assert_eq!(fields["SOCKSPort"], None);
        }
    }
    pub(crate) fn enable(path_journal: &std::path::Path, host: &str, port: u16) -> Result<()> {
        let host = if matches!(host, "0.0.0.0" | "::") {
            "127.0.0.1"
        } else {
            host.trim()
        };
        ensure!(!host.is_empty() && port != 0, "Invalid proxy endpoint");
        let old = super::super::read(path_journal)?;
        if let Some(old) = &old {
            ensure!(
                old.owner_pid == std::process::id() || !super::super::listener_alive(old),
                "System proxy is managed by another running instance"
            );
        }
        let prefs = Prefs::open(true, true)?;
        let services = prefs
            .get("/NetworkServices")
            .context("No network services")?;
        let (ids, values) = services.get_keys_and_values();
        let desired = desired(host, port)?;
        let mut snapshot = Snapshot {
            version: 2,
            services: Vec::new(),
        };
        let mut staged = Vec::new();
        for (id, value) in ids.into_iter().zip(values) {
            let id = unsafe { CFString::wrap_under_get_rule(id.cast()) }.to_string();
            let service = as_dict(unsafe { CFType::wrap_under_get_rule(value) })
                .context("Invalid network service")?;
            if field(&service, "__INACTIVE__").is_some() {
                continue;
            }
            let Some(interface) = field(&service, "Interface").and_then(as_dict) else {
                continue;
            };
            let kind = field(&interface, "Type")
                .and_then(|v| v.downcast::<CFString>())
                .map(|s| s.to_string())
                .unwrap_or_default();
            if !matches!(kind.as_str(), "Ethernet" | "IEEE80211") {
                continue;
            }
            let current_option = prefs.get(&path(&id));
            let before_exists = old
                .as_ref()
                .and_then(|j| j.macos.as_ref())
                .and_then(|s| s.services.iter().find(|s| s.id == id))
                .map(|s| s.before_exists)
                .unwrap_or(current_option.is_some());
            let current = current_option.unwrap_or_else(empty);
            let now = capture(&current)?;
            let before = old
                .as_ref()
                .and_then(|j| j.macos.as_ref())
                .and_then(|s| s.services.iter().find(|s| s.id == id))
                .map(|owned| restoration(&now, owned))
                .unwrap_or_else(|| now.clone());
            snapshot.services.push(Service {
                id: id.clone(),
                before_exists,
                before,
                applied: desired.clone(),
            });
            staged.push((id, overlay(&current, &desired)?));
        }
        ensure!(!staged.is_empty(), "No supported active network services");
        if let Some(previous) = old.as_ref().and_then(|j| j.macos.as_ref()) {
            for owned in &previous.services {
                if !snapshot.services.iter().any(|s| s.id == owned.id) {
                    snapshot.services.push(owned.clone());
                }
            }
        }
        let journal = super::super::Journal {
            owner_pid: std::process::id(),
            previous: old
                .as_ref()
                .map(|j| j.previous.clone())
                .unwrap_or_else(crate::system_proxy::SystemProxyConfig::disabled),
            applied: crate::system_proxy::SystemProxyConfig {
                enabled: true,
                host: host.into(),
                port,
                bypass: crate::system_proxy::PLATFORM_BYPASS.into(),
            },
            macos: Some(snapshot),
        };
        crate::config::storage::atomic_write_private(
            path_journal,
            &serde_yaml::to_string(&journal)?,
            false,
        )?;
        for (id, value) in &staged {
            prefs.set(&path(id), value)?;
        }
        prefs.commit()?;
        // A fresh preferences session confirms persisted values, not staged memory.
        drop(prefs);
        let verify = Prefs::open(false, false)?;
        for (id, _) in staged {
            ensure!(
                capture(
                    &verify
                        .get(&path(&id))
                        .context("Network service disappeared")?
                )? == desired,
                "Proxy change was not confirmed; recovery record retained"
            );
        }
        Ok(())
    }
    pub(crate) fn restore(
        path_journal: &std::path::Path,
        journal: &super::super::Journal,
    ) -> Result<()> {
        let snapshot = journal.macos.as_ref().context("Missing service snapshot")?;
        ensure!(snapshot.version == 2, "Unsupported proxy snapshot version");
        let prefs = Prefs::open(true, false)?;
        let mut expected = Vec::new();
        for owned in &snapshot.services {
            if let Some(current) = prefs.get(&path(&owned.id)) {
                let now = capture(&current)?;
                let restored = restoration(&now, owned);
                let result = overlay(&current, &restored)?;
                if !owned.before_exists && result.is_empty() {
                    prefs.remove(&path(&owned.id))?;
                } else {
                    prefs.set(&path(&owned.id), &result)?;
                }
                expected.push((owned.id.clone(), restored));
            }
        }
        prefs.commit()?;
        drop(prefs);
        let verify = Prefs::open(false, false)?;
        for (id, value) in expected {
            ensure!(
                capture(&verify.get(&path(&id)).unwrap_or_else(empty))? == value,
                "Proxy restore was not confirmed; recovery record retained"
            );
        }
        std::fs::remove_file(path_journal)?;
        let _ = crate::system_proxy::flush_system_dns_cache();
        Ok(())
    }
}
#[cfg(target_os = "macos")]
pub(crate) use native::import;
#[cfg(target_os = "macos")]
pub(super) use native::{enable, restore};

#[cfg(test)]
mod tests {
    use super::*;
    fn map(port: &str) -> BTreeMap<String, Option<String>> {
        GROUPS
            .iter()
            .flat_map(|group| group.iter())
            .map(|key| ((*key).into(), Some(format!("{key}:{port}"))))
            .collect()
    }
    #[test]
    fn distinct_protocol_settings_restore_and_external_changes_survive() {
        let mut before = map("before");
        before.insert("HTTPPort".into(), Some("8080".into()));
        before.insert("HTTPSPort".into(), Some("8443".into()));
        before.insert("SOCKSPort".into(), None);
        let owned = Service {
            id: "synthetic-service".into(),
            before_exists: true,
            before: before.clone(),
            applied: map("app"),
        };
        assert_eq!(restoration(&owned.applied, &owned), before);
        let mut current = owned.applied.clone();
        current.insert("HTTPSProxy".into(), Some("external".into()));
        let result = restoration(&current, &owned);
        assert_eq!(result.get("HTTPSProxy"), current.get("HTTPSProxy"));
        assert_eq!(result.get("HTTPSPort"), current.get("HTTPSPort"));
        assert_eq!(result.get("HTTPPort"), before.get("HTTPPort"));
        assert_eq!(result.get("SOCKSPort"), Some(&None));
    }
    #[test]
    fn journals_keep_network_service_identity() {
        let snapshot = Snapshot {
            version: 2,
            services: vec![
                Service {
                    id: "service-a".into(),
                    before_exists: false,
                    before: map("a"),
                    applied: map("app"),
                },
                Service {
                    id: "service-b".into(),
                    before_exists: true,
                    before: map("b"),
                    applied: map("app"),
                },
            ],
        };
        let restored: Snapshot =
            serde_yaml::from_str(&serde_yaml::to_string(&snapshot).unwrap()).unwrap();
        assert_ne!(restored.services[0].before, restored.services[1].before);
        assert!(!restored.services[0].before_exists);
    }
}
