//! RegKit — Windows registry read/write/list/delete behind a
//! small C ABI, with undo manifests in the FileOpsKit
//! discipline: every mutation first records the prior state
//! (old value, or the fact that a value/key did not exist),
//! and `rg_apply_undo` plays that record back to restore it.
//!
//! Structure:
//! - This module is the pure logic core: spec parsing, typed
//!   value conversion (JSON <-> `RegValue`), manifest build
//!   and apply, and the operations themselves — all written
//!   against the [`Backend`] trait so they are fully testable
//!   on any host against an in-memory fake.
//! - `platform` supplies the real backend: advapi32 on
//!   Windows, a clean ERR_NOT_SUPPORTED stub elsewhere.
//! - `ffi` is the C ABI surface declared in
//!   include/regkit.h.
//! - `json` is the family hand-rolled JSON engine (duplicate
//!   keys in one object are a parse error, by design).

pub mod ffi;
pub mod json;
pub mod platform;

#[cfg(test)]
pub mod fake;

pub const VERSION: &str = "0.1.0";
pub const VERSION_MAJOR: u32 = 0;
pub const VERSION_MINOR: u32 = 1;
pub const VERSION_PATCH: u32 = 0;

// Error codes (mirror include/regkit.h).
pub const RG_OK: i32 = 0;
pub const ERR_INVALID_ARG: i32 = 1;
pub const ERR_NOT_SUPPORTED: i32 = 2;
pub const ERR_NOT_FOUND: i32 = 3;
pub const ERR_ACCESS_DENIED: i32 = 4;
pub const ERR_PLATFORM: i32 = 5;
pub const ERR_INTERNAL: i32 = 6;

pub type RgResult<T> = Result<T, (i32, String)>;

use json::Value;

// ---------------------------------------------------------------------------
// Roots, values, backend
// ---------------------------------------------------------------------------

/// One of the four registry roots the API accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Root {
    Hkcu,
    Hklm,
    Hkcr,
    Hku,
}

impl Root {
    pub fn parse(s: &str) -> RgResult<Root> {
        match s {
            "HKCU" => Ok(Root::Hkcu),
            "HKLM" => Ok(Root::Hklm),
            "HKCR" => Ok(Root::Hkcr),
            "HKU" => Ok(Root::Hku),
            other => Err((
                ERR_INVALID_ARG,
                format!("unknown root \"{other}\" (expected HKCU, HKLM, HKCR or HKU)"),
            )),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Root::Hkcu => "HKCU",
            Root::Hklm => "HKLM",
            Root::Hkcr => "HKCR",
            Root::Hku => "HKU",
        }
    }
}

/// A typed registry value. The JSON representation (see
/// `value_json` / `value_from_json`):
/// sz/expand_sz -> string, dword/qword -> number,
/// multi_sz -> array of strings, binary -> base64 string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegValue {
    Sz(String),
    ExpandSz(String),
    Dword(u32),
    Qword(u64),
    MultiSz(Vec<String>),
    Binary(Vec<u8>),
}

impl RegValue {
    pub fn type_name(&self) -> &'static str {
        match self {
            RegValue::Sz(_) => "sz",
            RegValue::ExpandSz(_) => "expand_sz",
            RegValue::Dword(_) => "dword",
            RegValue::Qword(_) => "qword",
            RegValue::MultiSz(_) => "multi_sz",
            RegValue::Binary(_) => "binary",
        }
    }

    /// Serialize just the value (not the {"type":...} wrapper)
    /// as a JSON fragment. Numbers are emitted from the native
    /// integer so a qword prints exactly.
    pub fn to_json_fragment(&self) -> String {
        match self {
            RegValue::Sz(s) | RegValue::ExpandSz(s) => Value::Str(s.clone()).to_json(),
            RegValue::Dword(d) => d.to_string(),
            RegValue::Qword(q) => q.to_string(),
            RegValue::MultiSz(items) => {
                Value::Array(items.iter().map(|s| Value::Str(s.clone())).collect()).to_json()
            }
            RegValue::Binary(bytes) => Value::Str(base64_encode(bytes)).to_json(),
        }
    }

    /// The {"type": ..., "value": ...} object used by reads,
    /// listings and manifests.
    pub fn to_json(&self) -> String {
        format!(
            "{{\"type\":\"{}\",\"value\":{}}}",
            self.type_name(),
            self.to_json_fragment()
        )
    }
}

/// Parse a typed value from its JSON form. `type_name` is one
/// of the six type strings; `v` is the "value" member.
pub fn value_from_json(type_name: &str, v: &Value) -> RgResult<RegValue> {
    let bad = |want: &str| {
        (
            ERR_INVALID_ARG,
            format!(
                "\"value\" for type \"{type_name}\" must be {want}, got {}",
                v.type_name()
            ),
        )
    };
    match type_name {
        "sz" => v
            .as_str()
            .map(|s| RegValue::Sz(s.to_string()))
            .ok_or_else(|| bad("a string")),
        "expand_sz" => v
            .as_str()
            .map(|s| RegValue::ExpandSz(s.to_string()))
            .ok_or_else(|| bad("a string")),
        "dword" => match v.as_i64() {
            Some(n) if (0..=u32::MAX as i64).contains(&n) => Ok(RegValue::Dword(n as u32)),
            _ => Err((
                ERR_INVALID_ARG,
                "\"value\" for type \"dword\" must be an integer 0..=4294967295".to_string(),
            )),
        },
        "qword" => match v.as_i64() {
            Some(n) if n >= 0 => Ok(RegValue::Qword(n as u64)),
            _ => Err((
                ERR_INVALID_ARG,
                "\"value\" for type \"qword\" must be a non-negative integer (JSON numbers \
                 above 9223372036854775807 are not supported)"
                    .to_string(),
            )),
        },
        "multi_sz" => {
            let items = v.as_array().ok_or_else(|| bad("an array of strings"))?;
            let mut out = Vec::with_capacity(items.len());
            for it in items {
                out.push(
                    it.as_str()
                        .ok_or_else(|| bad("an array of strings"))?
                        .to_string(),
                );
            }
            Ok(RegValue::MultiSz(out))
        }
        "binary" => {
            let s = v.as_str().ok_or_else(|| bad("a base64 string"))?;
            let bytes = base64_decode(s).map_err(|e| {
                (
                    ERR_INVALID_ARG,
                    format!("\"value\" for type \"binary\" is not valid base64: {e}"),
                )
            })?;
            Ok(RegValue::Binary(bytes))
        }
        other => Err((ERR_INVALID_ARG, format!("unknown value type \"{other}\""))),
    }
}

/// One key's contents, as returned by `Backend::list`.
/// Value names use "" for the key's default value (the
/// Win32 convention).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyListing {
    pub subkeys: Vec<String>,
    pub values: Vec<(String, RegValue)>,
}

/// The storage backend the operations run against. The real
/// implementation talks to advapi32; tests substitute an
/// in-memory fake. Implementations must honour these
/// contracts:
/// - `read_value` with `name == None` reads the default
///   value; Ok(None) means the key exists but the value is
///   not set. A missing key is ERR_NOT_FOUND.
/// - `delete_key` removes ONE key including its values and
///   must fail if the key still has subkeys.
/// - `create_key` creates the full path (all missing
///   ancestors) and succeeds if the key already exists.
/// - `set_value` creates the key path if needed.
pub trait Backend {
    fn key_exists(&self, root: Root, path: &str) -> RgResult<bool>;
    fn read_value(&self, root: Root, path: &str, name: Option<&str>) -> RgResult<Option<RegValue>>;
    fn list(&self, root: Root, path: &str) -> RgResult<KeyListing>;
    fn create_key(&self, root: Root, path: &str) -> RgResult<()>;
    fn set_value(
        &self,
        root: Root,
        path: &str,
        name: Option<&str>,
        value: &RegValue,
    ) -> RgResult<()>;
    fn delete_value(&self, root: Root, path: &str, name: Option<&str>) -> RgResult<()>;
    fn delete_key(&self, root: Root, path: &str) -> RgResult<()>;
}

// ---------------------------------------------------------------------------
// Base64 (RFC 4648, standard alphabet, padded)
// ---------------------------------------------------------------------------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0] as u32,
            *chunk.get(1).unwrap_or(&0) as u32,
            *chunk.get(2).unwrap_or(&0) as u32,
        ];
        let n = (b[0] << 16) | (b[1] << 8) | b[2];
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

pub fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if bytes.len() % 4 != 0 {
        return Err(format!(
            "length {} is not a multiple of 4 (padding is required)",
            bytes.len()
        ));
    }
    let mut vals = Vec::with_capacity(bytes.len());
    for (i, &c) in bytes.iter().enumerate() {
        match c {
            b'A'..=b'Z' => vals.push(c - b'A'),
            b'a'..=b'z' => vals.push(c - b'a' + 26),
            b'0'..=b'9' => vals.push(c - b'0' + 52),
            b'+' => vals.push(62),
            b'/' => vals.push(63),
            b'=' => {
                // Padding only in the final two positions, and
                // nothing but padding after the first '='.
                if i < bytes.len() - 2 || bytes[i..].iter().any(|&x| x != b'=') {
                    return Err("misplaced '=' padding".to_string());
                }
                vals.push(0);
            }
            _ => return Err(format!("invalid character '{}'", c as char)),
        }
    }
    let pad = bytes.iter().rev().take_while(|&&c| c == b'=').count();
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in vals.chunks(4) {
        let n = ((chunk[0] as u32) << 18)
            | ((chunk[1] as u32) << 12)
            | ((chunk[2] as u32) << 6)
            | chunk[3] as u32;
        out.push((n >> 16) as u8);
        out.push((n >> 8) as u8);
        out.push(n as u8);
    }
    out.truncate(out.len() - pad);
    Ok(out)
}

// ---------------------------------------------------------------------------
// Spec parsing
// ---------------------------------------------------------------------------

fn parse_spec_json(text: &str) -> RgResult<Value> {
    let v = json::parse(text).map_err(|e| (ERR_INVALID_ARG, format!("invalid spec JSON: {e}")))?;
    if v.as_object().is_none() {
        return Err((ERR_INVALID_ARG, "spec must be a JSON object".to_string()));
    }
    Ok(v)
}

fn check_keys(v: &Value, known: &[&str]) -> RgResult<()> {
    for (k, _) in v.as_object().unwrap() {
        if !known.contains(&k.as_str()) {
            return Err((ERR_INVALID_ARG, format!("unknown spec key \"{k}\"")));
        }
    }
    Ok(())
}

fn spec_root_path(v: &Value) -> RgResult<(Root, String)> {
    let root_s = v.get("root").and_then(Value::as_str).ok_or((
        ERR_INVALID_ARG,
        "spec requires \"root\" (HKCU, HKLM, HKCR or HKU)".to_string(),
    ))?;
    let root = Root::parse(root_s)?;
    let path = v.get("path").and_then(Value::as_str).ok_or((
        ERR_INVALID_ARG,
        "spec requires \"path\" (a key path string)".to_string(),
    ))?;
    validate_path(path)?;
    Ok((root, path.to_string()))
}

/// Key paths are backslash-separated, non-empty, with no
/// empty segments (no leading/trailing/doubled backslash).
fn validate_path(path: &str) -> RgResult<()> {
    if path.is_empty() {
        return Err((ERR_INVALID_ARG, "\"path\" must not be empty".to_string()));
    }
    if path.split('\\').any(|seg| seg.is_empty()) {
        return Err((
            ERR_INVALID_ARG,
            format!("\"path\" has an empty segment: \"{path}\""),
        ));
    }
    Ok(())
}

/// The optional "name" member: None (or "") selects the
/// key's default value.
fn spec_name(v: &Value) -> RgResult<Option<String>> {
    match v.get("name") {
        None | Some(Value::Null) => Ok(None),
        Some(val) => match val.as_str() {
            Some(s) => Ok(Some(s.to_string())),
            None => Err((ERR_INVALID_ARG, "\"name\" must be a string".to_string())),
        },
    }
}

fn name_ref(name: &Option<String>) -> Option<&str> {
    match name {
        None => None,
        Some(s) if s.is_empty() => None,
        Some(s) => Some(s.as_str()),
    }
}

// ---------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------

/// rg_read core. With a name: that value's {"type","value"}.
/// Without: the key's default value, same shape.
pub fn read_with(b: &dyn Backend, spec_json: &str) -> RgResult<String> {
    let v = parse_spec_json(spec_json)?;
    check_keys(&v, &["root", "path", "name"])?;
    let (root, path) = spec_root_path(&v)?;
    let name = spec_name(&v)?;
    match b.read_value(root, &path, name_ref(&name))? {
        Some(value) => Ok(value.to_json()),
        None => Err((
            ERR_NOT_FOUND,
            match &name {
                Some(n) if !n.is_empty() => {
                    format!("value \"{n}\" is not set at {}\\{path}", root.as_str())
                }
                _ => format!("the default value of {}\\{path} is not set", root.as_str()),
            },
        )),
    }
}

/// rg_list core: {"subkeys":[...],"values":[{name,type,value}]}.
pub fn list_with(b: &dyn Backend, spec_json: &str) -> RgResult<String> {
    let v = parse_spec_json(spec_json)?;
    check_keys(&v, &["root", "path"])?;
    let (root, path) = spec_root_path(&v)?;
    let listing = b.list(root, &path)?;
    let subkeys: Vec<String> = listing
        .subkeys
        .iter()
        .map(|s| Value::Str(s.clone()).to_json())
        .collect();
    let values: Vec<String> = listing
        .values
        .iter()
        .map(|(name, val)| {
            format!(
                "{{\"name\":{},\"type\":\"{}\",\"value\":{}}}",
                Value::Str(name.clone()).to_json(),
                val.type_name(),
                val.to_json_fragment()
            )
        })
        .collect();
    Ok(format!(
        "{{\"subkeys\":[{}],\"values\":[{}]}}",
        subkeys.join(","),
        values.join(",")
    ))
}

/// The ancestor key paths of `path` that do not exist yet,
/// shallowest first. Creating `path` means creating exactly
/// these (create_key makes the whole chain at once; the list
/// exists so the undo manifest can name every created key).
fn missing_ancestors(b: &dyn Backend, root: Root, path: &str) -> RgResult<Vec<String>> {
    let mut missing = Vec::new();
    let mut cur = String::new();
    for seg in path.split('\\') {
        if !cur.is_empty() {
            cur.push('\\');
        }
        cur.push_str(seg);
        if !b.key_exists(root, &cur)? {
            missing.push(cur.clone());
        }
    }
    Ok(missing)
}

/// rg_write core: create the key path if needed, set the
/// value, and return {"undo_manifest": "..."} recording the
/// prior state (see manifest docs at `apply_undo_with`).
pub fn write_with(b: &dyn Backend, spec_json: &str) -> RgResult<String> {
    let v = parse_spec_json(spec_json)?;
    check_keys(&v, &["root", "path", "name", "type", "value"])?;
    let (root, path) = spec_root_path(&v)?;
    let name = spec_name(&v)?;
    let type_name = v.get("type").and_then(Value::as_str).ok_or((
        ERR_INVALID_ARG,
        "spec requires \"type\" (sz, expand_sz, dword, qword, multi_sz or binary)".to_string(),
    ))?;
    let raw_value = v
        .get("value")
        .ok_or((ERR_INVALID_ARG, "spec requires \"value\"".to_string()))?;
    let new_value = value_from_json(type_name, raw_value)?;

    // Capture prior state BEFORE touching anything (the
    // FileOpsKit discipline: the undo record describes the
    // world as it was, never as the operation leaves it).
    let created = missing_ancestors(b, root, &path)?;
    let prior = if created.is_empty() {
        b.read_value(root, &path, name_ref(&name))?
    } else {
        None
    };
    check_manifest_representable(prior.as_ref())?;

    b.create_key(root, &path)?;
    b.set_value(root, &path, name_ref(&name), &new_value)?;

    let mut entries: Vec<String> = created
        .iter()
        .map(|p| {
            format!(
                "{{\"action\":\"key_remove\",\"path\":{}}}",
                Value::Str(p.clone()).to_json()
            )
        })
        .collect();
    entries.push(value_entry(&path, &name, prior.as_ref()));
    let manifest = manifest_json(root, &entries);
    Ok(result_with_manifest(&manifest))
}

/// rg_delete core: delete one value, or one key (whole tree
/// only with "recursive": true). Returns the undo manifest.
pub fn delete_with(b: &dyn Backend, spec_json: &str) -> RgResult<String> {
    let v = parse_spec_json(spec_json)?;
    check_keys(&v, &["root", "path", "name", "recursive"])?;
    let (root, path) = spec_root_path(&v)?;
    let name = spec_name(&v)?;
    let recursive = match v.get("recursive") {
        None | Some(Value::Null) => false,
        Some(val) => val.as_bool().ok_or((
            ERR_INVALID_ARG,
            "\"recursive\" must be a boolean".to_string(),
        ))?,
    };

    // A "name" member (even "") selects a VALUE delete; its
    // absence selects a KEY delete.
    if name.is_some() {
        let n = name.as_deref().unwrap_or("");
        // Value delete.
        if !b.key_exists(root, &path)? {
            return Err(not_found_key(root, &path));
        }
        let prior = b.read_value(root, &path, name_ref(&name))?;
        let prior = prior.ok_or_else(|| {
            (
                ERR_NOT_FOUND,
                format!("value \"{n}\" is not set at {}\\{path}", root.as_str()),
            )
        })?;
        check_manifest_representable(Some(&prior))?;
        b.delete_value(root, &path, name_ref(&name))?;
        let entries = vec![value_entry(&path, &name, Some(&prior))];
        return Ok(result_with_manifest(&manifest_json(root, &entries)));
    }

    // Key delete.
    if !b.key_exists(root, &path)? {
        return Err(not_found_key(root, &path));
    }
    let listing = b.list(root, &path)?;
    if !recursive && !listing.subkeys.is_empty() {
        return Err((
            ERR_INVALID_ARG,
            format!(
                "key {}\\{path} has {} subkey(s); pass \"recursive\": true to delete the whole tree",
                root.as_str(),
                listing.subkeys.len()
            ),
        ));
    }
    // Snapshot the whole subtree BEFORE deleting anything,
    // parents before children.
    let mut snapshot: Vec<(String, KeyListing)> = Vec::new();
    snapshot_subtree(b, root, &path, &mut snapshot)?;
    for (_, l) in &snapshot {
        for (_, val) in &l.values {
            check_manifest_representable(Some(val))?;
        }
    }
    // Delete deepest first so each delete_key sees a key
    // with no subkeys.
    for (p, _) in snapshot.iter().rev() {
        b.delete_key(root, p)?;
    }
    let mut entries: Vec<String> = Vec::new();
    for (p, l) in &snapshot {
        entries.push(format!(
            "{{\"action\":\"key_create\",\"path\":{}}}",
            Value::Str(p.clone()).to_json()
        ));
        for (vname, val) in &l.values {
            entries.push(value_entry(p, &Some(vname.clone()), Some(val)));
        }
    }
    Ok(result_with_manifest(&manifest_json(root, &entries)))
}

fn snapshot_subtree(
    b: &dyn Backend,
    root: Root,
    path: &str,
    out: &mut Vec<(String, KeyListing)>,
) -> RgResult<()> {
    let listing = b.list(root, path)?;
    let subkeys = listing.subkeys.clone();
    out.push((path.to_string(), listing));
    for sub in subkeys {
        snapshot_subtree(b, root, &format!("{path}\\{sub}"), out)?;
    }
    Ok(())
}

/// rg_apply_undo core. The manifest is the JSON document
/// embedded in rg_write / rg_delete results:
///
///   {"version":1,"kind":"regkit-undo","root":"HKCU",
///    "entries":[ ... ]}
///
/// Entries are undo primitives, stored in the order they
/// were recorded and applied here in REVERSE order:
/// - {"action":"key_remove","path":P}  — delete key P
///   (skipped if P is already gone)
/// - {"action":"key_create","path":P}  — ensure key P exists
/// - {"action":"value_remove","path":P,"name":N} — delete
///   the value (skipped if already gone)
/// - {"action":"value_restore","path":P,"name":N,"type":T,
///   "value":V} — set the value to the recorded prior
///   value (creating P if needed)
///
/// Value entries recorded for a value that did not exist
/// before use "value_remove"; ones for a value that did use
/// "value_restore". Undo stops at the first failing entry
/// and reports it; entries already applied stay applied.
pub fn apply_undo_with(b: &dyn Backend, spec_json: &str) -> RgResult<()> {
    let v = parse_spec_json(spec_json)?;
    check_keys(&v, &["manifest"])?;
    let manifest_text = v.get("manifest").and_then(Value::as_str).ok_or((
        ERR_INVALID_ARG,
        "spec requires \"manifest\" (the undo_manifest string from a write/delete result)"
            .to_string(),
    ))?;
    let m = json::parse(manifest_text)
        .map_err(|e| (ERR_INVALID_ARG, format!("invalid undo manifest: {e}")))?;
    if m.as_object().is_none() {
        return Err((
            ERR_INVALID_ARG,
            "undo manifest must be a JSON object".to_string(),
        ));
    }
    match m.get("version").and_then(Value::as_i64) {
        Some(1) => {}
        _ => {
            return Err((
                ERR_INVALID_ARG,
                "undo manifest \"version\" must be 1".to_string(),
            ))
        }
    }
    let root_s = m.get("root").and_then(Value::as_str).ok_or((
        ERR_INVALID_ARG,
        "undo manifest is missing \"root\"".to_string(),
    ))?;
    let root = Root::parse(root_s).map_err(|_| {
        (
            ERR_INVALID_ARG,
            format!("undo manifest has unknown root \"{root_s}\""),
        )
    })?;
    let entries = m.get("entries").and_then(Value::as_array).ok_or((
        ERR_INVALID_ARG,
        "undo manifest has no \"entries\" array".to_string(),
    ))?;
    for entry in entries.iter().rev() {
        apply_undo_entry(b, root, entry)?;
    }
    Ok(())
}

fn apply_undo_entry(b: &dyn Backend, root: Root, entry: &Value) -> RgResult<()> {
    let action = entry.get("action").and_then(Value::as_str).ok_or((
        ERR_INVALID_ARG,
        "undo manifest entry is missing \"action\"".to_string(),
    ))?;
    let path = entry.get("path").and_then(Value::as_str).ok_or((
        ERR_INVALID_ARG,
        "undo manifest entry is missing \"path\"".to_string(),
    ))?;
    validate_path(path)?;
    let fail = |what: &str, e: (i32, String)| (e.0, format!("undo step {what} at {path}: {}", e.1));
    match action {
        "key_remove" => {
            if b.key_exists(root, path)? {
                b.delete_key(root, path)
                    .map_err(|e| fail("key_remove", e))?;
            }
            Ok(())
        }
        "key_create" => b.create_key(root, path).map_err(|e| fail("key_create", e)),
        "value_remove" | "value_restore" => {
            // An absent/null "name" means the default value;
            // a non-string name is a malformed manifest.
            let name: Option<String> = match entry.get("name") {
                None | Some(Value::Null) => None,
                Some(val) => Some(
                    val.as_str()
                        .ok_or((
                            ERR_INVALID_ARG,
                            "undo manifest value entry \"name\" must be a string".to_string(),
                        ))?
                        .to_string(),
                ),
            };
            if action == "value_remove" {
                if b.key_exists(root, path)? && b.read_value(root, path, name_ref(&name))?.is_some()
                {
                    b.delete_value(root, path, name_ref(&name))
                        .map_err(|e| fail("value_remove", e))?;
                }
                Ok(())
            } else {
                let type_name = entry.get("type").and_then(Value::as_str).ok_or((
                    ERR_INVALID_ARG,
                    "undo manifest value_restore entry is missing \"type\"".to_string(),
                ))?;
                let raw = entry.get("value").ok_or((
                    ERR_INVALID_ARG,
                    "undo manifest value_restore entry is missing \"value\"".to_string(),
                ))?;
                let value = value_from_json(type_name, raw)?;
                b.create_key(root, path)
                    .map_err(|e| fail("value_restore (create key)", e))?;
                b.set_value(root, path, name_ref(&name), &value)
                    .map_err(|e| fail("value_restore", e))
            }
        }
        other => Err((ERR_INVALID_ARG, format!("unknown undo action \"{other}\""))),
    }
}

// ---------------------------------------------------------------------------
// Manifest helpers
// ---------------------------------------------------------------------------

/// A prior qword above i64::MAX cannot round-trip through
/// the JSON parser exactly, so a manifest holding it could
/// not be trusted. Refuse the operation up front (before
/// changing anything) rather than record a lossy undo.
fn check_manifest_representable(prior: Option<&RegValue>) -> RgResult<()> {
    if let Some(RegValue::Qword(q)) = prior {
        if *q > i64::MAX as u64 {
            return Err((
                ERR_PLATFORM,
                "the existing qword value exceeds 9223372036854775807 and cannot be \
                 recorded exactly in an undo manifest; refusing to change it"
                    .to_string(),
            ));
        }
    }
    Ok(())
}

/// Build one manifest value entry. `prior == None` records
/// that the value did not exist (undo removes it); the
/// "value_remove" action carries no type/value members.
fn value_entry(path: &str, name: &Option<String>, prior: Option<&RegValue>) -> String {
    let name_json = Value::Str(name.clone().unwrap_or_default()).to_json();
    let path_json = Value::Str(path.to_string()).to_json();
    match prior {
        Some(val) => format!(
            "{{\"action\":\"value_restore\",\"path\":{path_json},\"name\":{name_json},\"type\":\"{}\",\"value\":{}}}",
            val.type_name(),
            val.to_json_fragment()
        ),
        None => format!(
            "{{\"action\":\"value_remove\",\"path\":{path_json},\"name\":{name_json}}}"
        ),
    }
}

fn manifest_json(root: Root, entries: &[String]) -> String {
    format!(
        "{{\"version\":1,\"kind\":\"regkit-undo\",\"root\":\"{}\",\"entries\":[{}]}}",
        root.as_str(),
        entries.join(",")
    )
}

fn result_with_manifest(manifest: &str) -> String {
    format!(
        "{{\"undo_manifest\":{}}}",
        Value::Str(manifest.to_string()).to_json()
    )
}

fn not_found_key(root: Root, path: &str) -> (i32, String) {
    (
        ERR_NOT_FOUND,
        format!("key not found: {}\\{path}", root.as_str()),
    )
}

// ---------------------------------------------------------------------------
// Backend-bound entry points (what the FFI calls)
// ---------------------------------------------------------------------------

pub fn read(spec_json: &str) -> RgResult<String> {
    read_with(&*platform::backend(), spec_json)
}

pub fn list(spec_json: &str) -> RgResult<String> {
    list_with(&*platform::backend(), spec_json)
}

pub fn write(spec_json: &str) -> RgResult<String> {
    write_with(&*platform::backend(), spec_json)
}

pub fn delete(spec_json: &str) -> RgResult<String> {
    delete_with(&*platform::backend(), spec_json)
}

pub fn apply_undo(spec_json: &str) -> RgResult<()> {
    apply_undo_with(&*platform::backend(), spec_json)
}

/// Escape a string for inclusion inside JSON double quotes.
pub fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests;
