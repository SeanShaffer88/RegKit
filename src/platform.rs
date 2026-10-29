//! Platform layer: the only place Windows APIs are touched.
//! Supplies the real `Backend` (advapi32) on Windows; on any
//! other target every operation returns ERR_NOT_SUPPORTED so
//! the pure logic and FFI contract stay testable anywhere.
//!
//! Every extern block MUST carry #[link(name = "...")] naming
//! its import library. Without it the build fails at LINK
//! time with LNK2019 — check and clippy cannot catch it; CI's
//! real build can.

use crate::{Backend, ERR_NOT_SUPPORTED};

#[cfg(not(windows))]
use crate::{KeyListing, RegValue, RgResult, Root};

pub type PResult<T> = Result<T, (i32, String)>;

pub fn not_supported<T>(what: &str) -> PResult<T> {
    Err((
        ERR_NOT_SUPPORTED,
        format!("{what} is only available on Windows"),
    ))
}

/// The backend the FFI entry points run against.
#[cfg(windows)]
pub fn backend() -> Box<dyn Backend> {
    Box::new(win::WinBackend)
}

#[cfg(not(windows))]
pub fn backend() -> Box<dyn Backend> {
    Box::new(UnsupportedBackend)
}

/// Off-Windows backend: every operation is a clean
/// ERR_NOT_SUPPORTED, never a panic or a fake success.
#[cfg(not(windows))]
struct UnsupportedBackend;

#[cfg(not(windows))]
impl Backend for UnsupportedBackend {
    fn key_exists(&self, _root: Root, _path: &str) -> RgResult<bool> {
        not_supported("registry access")
    }
    fn read_value(
        &self,
        _root: Root,
        _path: &str,
        _name: Option<&str>,
    ) -> RgResult<Option<RegValue>> {
        not_supported("registry access")
    }
    fn list(&self, _root: Root, _path: &str) -> RgResult<KeyListing> {
        not_supported("registry access")
    }
    fn create_key(&self, _root: Root, _path: &str) -> RgResult<()> {
        not_supported("registry access")
    }
    fn set_value(
        &self,
        _root: Root,
        _path: &str,
        _name: Option<&str>,
        _value: &RegValue,
    ) -> RgResult<()> {
        not_supported("registry access")
    }
    fn delete_value(&self, _root: Root, _path: &str, _name: Option<&str>) -> RgResult<()> {
        not_supported("registry access")
    }
    fn delete_key(&self, _root: Root, _path: &str) -> RgResult<()> {
        not_supported("registry access")
    }
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    use crate::{
        Backend, KeyListing, RegValue, RgResult, Root, ERR_ACCESS_DENIED, ERR_NOT_FOUND,
        ERR_NOT_SUPPORTED, ERR_PLATFORM,
    };

    type Hkey = *mut c_void;
    type Lstatus = i32;

    const ERROR_SUCCESS: i32 = 0;
    const ERROR_FILE_NOT_FOUND: i32 = 2;
    const ERROR_PATH_NOT_FOUND: i32 = 3;
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_NO_MORE_ITEMS: i32 = 259;

    const KEY_READ: u32 = 0x20019;
    const KEY_WRITE: u32 = 0x20006;
    const KEY_SET_VALUE: u32 = 0x0002;
    const DELETE_ACCESS: u32 = 0x0001_0000;

    const REG_SZ: u32 = 1;
    const REG_EXPAND_SZ: u32 = 2;
    const REG_BINARY: u32 = 3;
    const REG_DWORD: u32 = 4;
    const REG_MULTI_SZ: u32 = 7;
    const REG_QWORD: u32 = 11;

    #[link(name = "advapi32")]
    extern "system" {
        fn RegOpenKeyExW(
            hkey: Hkey,
            sub_key: *const u16,
            options: u32,
            sam_desired: u32,
            result: *mut Hkey,
        ) -> Lstatus;
        fn RegCreateKeyExW(
            hkey: Hkey,
            sub_key: *const u16,
            reserved: u32,
            class: *const u16,
            options: u32,
            sam_desired: u32,
            security: *const c_void,
            result: *mut Hkey,
            disposition: *mut u32,
        ) -> Lstatus;
        fn RegCloseKey(hkey: Hkey) -> Lstatus;
        fn RegQueryValueExW(
            hkey: Hkey,
            value_name: *const u16,
            reserved: *const u32,
            value_type: *mut u32,
            data: *mut u8,
            data_len: *mut u32,
        ) -> Lstatus;
        fn RegSetValueExW(
            hkey: Hkey,
            value_name: *const u16,
            reserved: u32,
            value_type: u32,
            data: *const u8,
            data_len: u32,
        ) -> Lstatus;
        fn RegDeleteValueW(hkey: Hkey, value_name: *const u16) -> Lstatus;
        fn RegDeleteKeyW(hkey: Hkey, sub_key: *const u16) -> Lstatus;
        fn RegEnumKeyExW(
            hkey: Hkey,
            index: u32,
            name: *mut u16,
            name_len: *mut u32,
            reserved: *const u32,
            class: *mut u16,
            class_len: *mut u32,
            last_write: *mut u64,
        ) -> Lstatus;
        fn RegEnumValueW(
            hkey: Hkey,
            index: u32,
            value_name: *mut u16,
            name_len: *mut u32,
            reserved: *const u32,
            value_type: *mut u32,
            data: *mut u8,
            data_len: *mut u32,
        ) -> Lstatus;
        fn RegQueryInfoKeyW(
            hkey: Hkey,
            class: *mut u16,
            class_len: *mut u32,
            reserved: *const u32,
            sub_keys: *mut u32,
            max_sub_key_len: *mut u32,
            max_class_len: *mut u32,
            values: *mut u32,
            max_value_name_len: *mut u32,
            max_value_len: *mut u32,
            security_descriptor: *mut u32,
            last_write: *mut u64,
        ) -> Lstatus;
    }

    fn root_handle(root: Root) -> Hkey {
        // The predefined keys are the well-known 0x8000000n
        // constants, sign-extended to pointer width.
        let bits: u32 = match root {
            Root::Hkcr => 0x8000_0000,
            Root::Hkcu => 0x8000_0001,
            Root::Hklm => 0x8000_0002,
            Root::Hku => 0x8000_0003,
        };
        (bits as i32 as isize) as Hkey
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Value-name pointer: NULL selects the default value
    /// (the Win32 convention); "" works too but NULL is the
    /// documented form.
    fn name_ptr(w: &Option<Vec<u16>>) -> *const u16 {
        match w {
            None => std::ptr::null(),
            Some(v) => v.as_ptr(),
        }
    }

    fn map_err(api: &str, status: Lstatus) -> (i32, String) {
        let code = match status {
            ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => ERR_NOT_FOUND,
            ERROR_ACCESS_DENIED => ERR_ACCESS_DENIED,
            _ => ERR_PLATFORM,
        };
        (code, format!("{api} failed (Windows error {status})"))
    }

    /// RAII for an opened/created key handle. Predefined
    /// root handles are never wrapped (never closed).
    struct KeyGuard(Hkey);

    impl Drop for KeyGuard {
        fn drop(&mut self) {
            // SAFETY: the handle came from a successful
            // open/create and is closed exactly once, here.
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }

    fn open_key(root: Root, path: &str, sam: u32) -> RgResult<KeyGuard> {
        let w = wide(path);
        let mut handle: Hkey = std::ptr::null_mut();
        // SAFETY: root_handle is a predefined key; `w` is a
        // valid NUL-terminated wide string; `handle` is an
        // out pointer owned by this frame.
        let status = unsafe { RegOpenKeyExW(root_handle(root), w.as_ptr(), 0, sam, &mut handle) };
        if status != ERROR_SUCCESS {
            return Err(map_err("RegOpenKeyExW", status));
        }
        Ok(KeyGuard(handle))
    }

    fn create_key_handle(root: Root, path: &str, sam: u32) -> RgResult<KeyGuard> {
        let w = wide(path);
        let mut handle: Hkey = std::ptr::null_mut();
        // SAFETY: same contract as open_key; NULL class /
        // security attributes, disposition not needed.
        let status = unsafe {
            RegCreateKeyExW(
                root_handle(root),
                w.as_ptr(),
                0,
                std::ptr::null(),
                0,
                sam,
                std::ptr::null(),
                &mut handle,
                std::ptr::null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(map_err("RegCreateKeyExW", status));
        }
        Ok(KeyGuard(handle))
    }

    /// Convert raw (type, bytes) from the registry to a
    /// typed value. Types outside the kit's six are a clean
    /// ERR_NOT_SUPPORTED, never a guess.
    fn bytes_to_value(ty: u32, data: &[u8]) -> RgResult<RegValue> {
        match ty {
            REG_SZ | REG_EXPAND_SZ => {
                let units: Vec<u16> = data
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                let s = String::from_utf16_lossy(&units)
                    .trim_end_matches('\0')
                    .to_string();
                Ok(if ty == REG_SZ {
                    RegValue::Sz(s)
                } else {
                    RegValue::ExpandSz(s)
                })
            }
            REG_DWORD => {
                if data.len() != 4 {
                    return Err((
                        ERR_PLATFORM,
                        format!("REG_DWORD with {} bytes (expected 4)", data.len()),
                    ));
                }
                Ok(RegValue::Dword(u32::from_le_bytes([
                    data[0], data[1], data[2], data[3],
                ])))
            }
            REG_QWORD => {
                if data.len() != 8 {
                    return Err((
                        ERR_PLATFORM,
                        format!("REG_QWORD with {} bytes (expected 8)", data.len()),
                    ));
                }
                Ok(RegValue::Qword(u64::from_le_bytes([
                    data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
                ])))
            }
            REG_MULTI_SZ => {
                let units: Vec<u16> = data
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                let mut items = Vec::new();
                let mut cur: Vec<u16> = Vec::new();
                for u in units {
                    if u == 0 {
                        if cur.is_empty() {
                            break; // the terminating empty string
                        }
                        items.push(String::from_utf16_lossy(&cur));
                        cur.clear();
                    } else {
                        cur.push(u);
                    }
                }
                if !cur.is_empty() {
                    items.push(String::from_utf16_lossy(&cur));
                }
                Ok(RegValue::MultiSz(items))
            }
            REG_BINARY => Ok(RegValue::Binary(data.to_vec())),
            other => Err((
                ERR_NOT_SUPPORTED,
                format!("registry value type {other} is not supported by RegKit"),
            )),
        }
    }

    fn value_to_bytes(value: &RegValue) -> (u32, Vec<u8>) {
        fn string_bytes(s: &str) -> Vec<u8> {
            s.encode_utf16()
                .chain(std::iter::once(0))
                .flat_map(|u| u.to_le_bytes())
                .collect()
        }
        match value {
            RegValue::Sz(s) => (REG_SZ, string_bytes(s)),
            RegValue::ExpandSz(s) => (REG_EXPAND_SZ, string_bytes(s)),
            RegValue::Dword(d) => (REG_DWORD, d.to_le_bytes().to_vec()),
            RegValue::Qword(q) => (REG_QWORD, q.to_le_bytes().to_vec()),
            RegValue::MultiSz(items) => {
                let mut units: Vec<u16> = Vec::new();
                for item in items {
                    units.extend(item.encode_utf16());
                    units.push(0);
                }
                units.push(0);
                (
                    REG_MULTI_SZ,
                    units.iter().flat_map(|u| u.to_le_bytes()).collect(),
                )
            }
            RegValue::Binary(bytes) => (REG_BINARY, bytes.clone()),
        }
    }

    fn query_value(key: &KeyGuard, name: Option<&str>) -> RgResult<Option<RegValue>> {
        let wname: Option<Vec<u16>> = name.map(wide);
        let mut ty: u32 = 0;
        let mut len: u32 = 0;
        // SAFETY: name pointer is NULL (default value) or a
        // valid wide string; data pointer NULL = size query.
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                name_ptr(&wname),
                std::ptr::null(),
                &mut ty,
                std::ptr::null_mut(),
                &mut len,
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if status != ERROR_SUCCESS {
            return Err(map_err("RegQueryValueExW", status));
        }
        let mut buf = vec![0u8; len as usize];
        let mut len2 = len;
        // SAFETY: buf holds exactly the reported size; the
        // second call re-validates it in len2.
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                name_ptr(&wname),
                std::ptr::null(),
                &mut ty,
                buf.as_mut_ptr(),
                &mut len2,
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if status != ERROR_SUCCESS {
            return Err(map_err("RegQueryValueExW", status));
        }
        buf.truncate(len2 as usize);
        bytes_to_value(ty, &buf).map(Some)
    }

    pub struct WinBackend;

    impl Backend for WinBackend {
        fn key_exists(&self, root: Root, path: &str) -> RgResult<bool> {
            match open_key(root, path, KEY_READ) {
                Ok(_) => Ok(true),
                Err((code, _)) if code == ERR_NOT_FOUND => Ok(false),
                Err(e) => Err(e),
            }
        }

        fn read_value(
            &self,
            root: Root,
            path: &str,
            name: Option<&str>,
        ) -> RgResult<Option<RegValue>> {
            let key = open_key(root, path, KEY_READ)?;
            query_value(&key, name)
        }

        fn list(&self, root: Root, path: &str) -> RgResult<KeyListing> {
            let key = open_key(root, path, KEY_READ)?;
            let mut sub_count: u32 = 0;
            let mut max_sub_len: u32 = 0;
            let mut val_count: u32 = 0;
            let mut max_name_len: u32 = 0;
            let mut max_val_len: u32 = 0;
            // SAFETY: all out pointers are owned locals;
            // class buffer not requested (NULL / 0).
            let status = unsafe {
                RegQueryInfoKeyW(
                    key.0,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    &mut sub_count,
                    &mut max_sub_len,
                    std::ptr::null_mut(),
                    &mut val_count,
                    &mut max_name_len,
                    &mut max_val_len,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if status != ERROR_SUCCESS {
                return Err(map_err("RegQueryInfoKeyW", status));
            }
            let mut subkeys = Vec::with_capacity(sub_count as usize);
            for i in 0..sub_count {
                let mut buf = vec![0u16; max_sub_len as usize + 2];
                let mut len = buf.len() as u32;
                // SAFETY: buffer sized from the reported
                // maximum (+ slack); len is in/out.
                let status = unsafe {
                    RegEnumKeyExW(
                        key.0,
                        i,
                        buf.as_mut_ptr(),
                        &mut len,
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                };
                if status == ERROR_NO_MORE_ITEMS {
                    break;
                }
                if status != ERROR_SUCCESS {
                    return Err(map_err("RegEnumKeyExW", status));
                }
                subkeys.push(String::from_utf16_lossy(&buf[..len as usize]));
            }
            let mut values = Vec::with_capacity(val_count as usize);
            for i in 0..val_count {
                let mut name_buf = vec![0u16; max_name_len as usize + 2];
                let mut name_len = name_buf.len() as u32;
                let mut ty: u32 = 0;
                let mut data_len: u32 = 0;
                // SAFETY: name buffer sized from the
                // reported maximum; data NULL = size query.
                let status = unsafe {
                    RegEnumValueW(
                        key.0,
                        i,
                        name_buf.as_mut_ptr(),
                        &mut name_len,
                        std::ptr::null(),
                        &mut ty,
                        std::ptr::null_mut(),
                        &mut data_len,
                    )
                };
                if status == ERROR_NO_MORE_ITEMS {
                    break;
                }
                if status != ERROR_SUCCESS {
                    return Err(map_err("RegEnumValueW", status));
                }
                let name = String::from_utf16_lossy(&name_buf[..name_len as usize]);
                // Read through query_value so conversion
                // lives in exactly one place. The default
                // value enumerates with name "" -> NULL ptr.
                let value = query_value(&key, if name.is_empty() { None } else { Some(&name) })?
                    .ok_or((
                        ERR_PLATFORM,
                        "enumerated value vanished before it could be read".to_string(),
                    ))?;
                values.push((name, value));
            }
            Ok(KeyListing { subkeys, values })
        }

        fn create_key(&self, root: Root, path: &str) -> RgResult<()> {
            let _guard = create_key_handle(root, path, KEY_WRITE)?;
            Ok(())
        }

        fn set_value(
            &self,
            root: Root,
            path: &str,
            name: Option<&str>,
            value: &RegValue,
        ) -> RgResult<()> {
            let key = create_key_handle(root, path, KEY_WRITE | KEY_SET_VALUE)?;
            let wname: Option<Vec<u16>> = name.map(wide);
            let (ty, bytes) = value_to_bytes(value);
            // SAFETY: data pointer/length describe `bytes`;
            // an empty binary passes a non-null dangling-free
            // pointer from the empty vec with length 0,
            // which RegSetValueExW accepts.
            let status = unsafe {
                RegSetValueExW(
                    key.0,
                    name_ptr(&wname),
                    0,
                    ty,
                    bytes.as_ptr(),
                    bytes.len() as u32,
                )
            };
            if status != ERROR_SUCCESS {
                return Err(map_err("RegSetValueExW", status));
            }
            Ok(())
        }

        fn delete_value(&self, root: Root, path: &str, name: Option<&str>) -> RgResult<()> {
            let key = open_key(root, path, KEY_WRITE | KEY_SET_VALUE)?;
            let wname: Option<Vec<u16>> = name.map(wide);
            // SAFETY: name pointer is NULL or a valid wide
            // string owned by this frame.
            let status = unsafe { RegDeleteValueW(key.0, name_ptr(&wname)) };
            if status != ERROR_SUCCESS {
                return Err(map_err("RegDeleteValueW", status));
            }
            Ok(())
        }

        fn delete_key(&self, root: Root, path: &str) -> RgResult<()> {
            // RegDeleteKeyW deletes a SUBKEY of an open key,
            // so split the path and open the parent (or use
            // the predefined root directly for a top-level
            // key). The parent needs DELETE access.
            let (parent_path, leaf) = match path.rfind('\\') {
                Some(i) => (Some(&path[..i]), &path[i + 1..]),
                None => (None, path),
            };
            let wleaf = wide(leaf);
            let status = match parent_path {
                Some(parent) => {
                    let pkey = open_key(root, parent, DELETE_ACCESS)?;
                    // SAFETY: leaf is a valid wide string;
                    // pkey is an open parent handle.
                    unsafe { RegDeleteKeyW(pkey.0, wleaf.as_ptr()) }
                }
                None => {
                    // SAFETY: root_handle is a predefined
                    // key; leaf is a valid wide string.
                    unsafe { RegDeleteKeyW(root_handle(root), wleaf.as_ptr()) }
                }
            };
            if status != ERROR_SUCCESS {
                return Err(map_err("RegDeleteKeyW", status));
            }
            Ok(())
        }
    }

    #[cfg(test)]
    mod smoke {
        //! Real-registry smoke test — Windows only. Runs a
        //! full roundtrip under a throwaway HKCU key and
        //! removes it again. CI on windows-latest is where
        //! this actually executes.
        use super::WinBackend;
        use crate::{
            apply_undo_with, delete_with, list_with, read_with, write_with, Backend, Root,
        };

        #[test]
        fn hkcu_throwaway_roundtrip() {
            let b = WinBackend;
            let base = format!("Software\\RegKitSmoke{}", std::process::id());
            let root = Root::Hkcu;
            // Best-effort cleanup of any earlier crashed run.
            if b.key_exists(root, &base).unwrap() {
                let _ = delete_with(
                    &b,
                    &format!(
                        "{{\"root\":\"HKCU\",\"path\":\"{}\",\"recursive\":true}}",
                        base.replace('\\', "\\\\")
                    ),
                );
            }
            let esc = base.replace('\\', "\\\\");
            let write = |spec: String| write_with(&b, &spec).unwrap();
            let undo_of = |result: &str| -> String {
                let v = crate::json::parse(result).unwrap();
                v.get("undo_manifest")
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string()
            };
            let undo_spec = |manifest: &str| {
                format!(
                    "{{\"manifest\":{}}}",
                    crate::json::Value::Str(manifest.to_string()).to_json()
                )
            };

            // Write one value of several types into a fresh tree.
            let r = write(format!(
                "{{\"root\":\"HKCU\",\"path\":\"{esc}\",\"name\":\"Greeting\",\"type\":\"sz\",\"value\":\"hello\"}}"
            ));
            let m1 = undo_of(&r);
            write(format!(
                "{{\"root\":\"HKCU\",\"path\":\"{esc}\",\"name\":\"Count\",\"type\":\"dword\",\"value\":7}}"
            ));
            write(format!(
                "{{\"root\":\"HKCU\",\"path\":\"{esc}\",\"name\":\"Tags\",\"type\":\"multi_sz\",\"value\":[\"a\",\"b\"]}}"
            ));
            write(format!(
                "{{\"root\":\"HKCU\",\"path\":\"{esc}\",\"name\":\"Blob\",\"type\":\"binary\",\"value\":\"AAE=\"}}"
            ));

            let got = read_with(
                &b,
                &format!("{{\"root\":\"HKCU\",\"path\":\"{esc}\",\"name\":\"Greeting\"}}"),
            )
            .unwrap();
            assert_eq!(got, "{\"type\":\"sz\",\"value\":\"hello\"}");
            let listing =
                list_with(&b, &format!("{{\"root\":\"HKCU\",\"path\":\"{esc}\"}}")).unwrap();
            assert!(listing.contains("\"Count\""), "{listing}");
            assert!(listing.contains("\"Blob\""), "{listing}");

            // Undo of the FIRST write: value_remove deletes
            // the Greeting value, then key_remove deletes the
            // created key itself — RegDeleteKey semantics
            // take the key's remaining values (Count, Tags)
            // with it, so the whole tree disappears.
            let _blob_delete = delete_with(
                &b,
                &format!("{{\"root\":\"HKCU\",\"path\":\"{esc}\",\"name\":\"Blob\"}}"),
            )
            .unwrap();
            apply_undo_with(&b, &undo_spec(&m1)).unwrap();
            assert!(!b.key_exists(root, &base).unwrap());
        }
    }
}
