//! Host tests: the operations run against the in-memory
//! fake backend, so manifest build/apply logic, type/value
//! conversions and the recursive-delete refusal are all
//! verified without a registry. The real-registry smoke
//! test lives in platform.rs (Windows only).

use super::fake::FakeBackend;
use super::*;

// -- helpers ---------------------------------------------------------------

fn jp(path: &str) -> String {
    path.replace('\\', "\\\\")
}

fn spec_read(path: &str, name: Option<&str>) -> String {
    match name {
        Some(n) => format!(
            "{{\"root\":\"HKCU\",\"path\":\"{}\",\"name\":\"{}\"}}",
            jp(path),
            n
        ),
        None => format!("{{\"root\":\"HKCU\",\"path\":\"{}\"}}", jp(path)),
    }
}

fn spec_write(path: &str, name: Option<&str>, ty: &str, value_json: &str) -> String {
    match name {
        Some(n) => format!(
            "{{\"root\":\"HKCU\",\"path\":\"{}\",\"name\":\"{}\",\"type\":\"{ty}\",\"value\":{value_json}}}",
            jp(path),
            n
        ),
        None => format!(
            "{{\"root\":\"HKCU\",\"path\":\"{}\",\"type\":\"{ty}\",\"value\":{value_json}}}",
            jp(path)
        ),
    }
}

fn manifest_of(result_json: &str) -> String {
    let v = json::parse(result_json).unwrap();
    v.get("undo_manifest")
        .and_then(Value::as_str)
        .expect("result must carry an undo_manifest string")
        .to_string()
}

fn undo_spec(manifest: &str) -> String {
    format!(
        "{{\"manifest\":{}}}",
        Value::Str(manifest.to_string()).to_json()
    )
}

fn read_val(b: &FakeBackend, path: &str, name: Option<&str>) -> String {
    read_with(b, &spec_read(path, name)).unwrap()
}

// -- base64 -----------------------------------------------------------------

#[test]
fn base64_known_vectors() {
    assert_eq!(base64_encode(b""), "");
    assert_eq!(base64_encode(b"f"), "Zg==");
    assert_eq!(base64_encode(b"fo"), "Zm8=");
    assert_eq!(base64_encode(b"foo"), "Zm9v");
    assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
    assert_eq!(base64_encode(&[0x00, 0x01, 0xfe, 0xff]), "AAH+/w==");
    assert_eq!(base64_decode("").unwrap(), Vec::<u8>::new());
    assert_eq!(base64_decode("Zg==").unwrap(), b"f");
    assert_eq!(base64_decode("Zm9v").unwrap(), b"foo");
    assert_eq!(
        base64_decode("AAH+/w==").unwrap(),
        vec![0x00, 0x01, 0xfe, 0xff]
    );
}

#[test]
fn base64_roundtrip_all_bytes() {
    let data: Vec<u8> = (0u16..=255).map(|n| n as u8).collect();
    let enc = base64_encode(&data);
    assert_eq!(base64_decode(&enc).unwrap(), data);
}

#[test]
fn base64_rejects_garbage() {
    assert!(base64_decode("Zg").is_err(), "unpadded must fail");
    assert!(base64_decode("Zg===").is_err(), "length not multiple of 4");
    assert!(base64_decode("Z!==").is_err(), "invalid character");
    assert!(base64_decode("====").is_err(), "padding-only");
    assert!(base64_decode("Zg==Zg==").is_err(), "padding mid-stream");
    assert!(base64_decode("Zm9v\n").is_err(), "whitespace not allowed");
}

// -- value conversion --------------------------------------------------------

#[test]
fn value_json_roundtrips() {
    let cases: Vec<(RegValue, &str, &str)> = vec![
        (
            RegValue::Sz("hi \"there\"".into()),
            "sz",
            "\"hi \\\"there\\\"\"",
        ),
        (
            RegValue::ExpandSz("%PATH%\\x".into()),
            "expand_sz",
            "\"%PATH%\\\\x\"",
        ),
        (RegValue::Dword(0), "dword", "0"),
        (RegValue::Dword(u32::MAX), "dword", "4294967295"),
        (RegValue::Qword(0), "qword", "0"),
        (
            RegValue::Qword(i64::MAX as u64),
            "qword",
            "9223372036854775807",
        ),
        (
            RegValue::MultiSz(vec!["a".into(), "b c".into()]),
            "multi_sz",
            "[\"a\",\"b c\"]",
        ),
        (RegValue::MultiSz(vec![]), "multi_sz", "[]"),
        (RegValue::Binary(vec![1, 2, 3]), "binary", "\"AQID\""),
        (RegValue::Binary(vec![]), "binary", "\"\""),
    ];
    for (val, ty, fragment) in cases {
        assert_eq!(val.to_json_fragment(), fragment, "encode {val:?}");
        let parsed = json::parse(fragment).unwrap();
        assert_eq!(value_from_json(ty, &parsed).unwrap(), val, "decode {ty}");
        assert_eq!(
            val.to_json(),
            format!("{{\"type\":\"{ty}\",\"value\":{fragment}}}")
        );
    }
}

#[test]
fn value_from_json_rejects_bad_shapes() {
    let j = |s: &str| json::parse(s).unwrap();
    assert_eq!(
        value_from_json("dword", &j("-1")).unwrap_err().0,
        ERR_INVALID_ARG
    );
    assert_eq!(
        value_from_json("dword", &j("4294967296")).unwrap_err().0,
        ERR_INVALID_ARG
    );
    assert_eq!(
        value_from_json("dword", &j("\"7\"")).unwrap_err().0,
        ERR_INVALID_ARG
    );
    assert_eq!(
        value_from_json("qword", &j("-5")).unwrap_err().0,
        ERR_INVALID_ARG
    );
    assert_eq!(
        value_from_json("sz", &j("3")).unwrap_err().0,
        ERR_INVALID_ARG
    );
    assert_eq!(
        value_from_json("multi_sz", &j("[\"a\",2]")).unwrap_err().0,
        ERR_INVALID_ARG
    );
    assert_eq!(
        value_from_json("binary", &j("\"!!\"")).unwrap_err().0,
        ERR_INVALID_ARG
    );
    assert_eq!(
        value_from_json("nope", &j("1")).unwrap_err().0,
        ERR_INVALID_ARG
    );
}

// -- spec validation ----------------------------------------------------------

#[test]
fn spec_validation() {
    let b = FakeBackend::new();
    let bad = |spec: &str| read_with(&b, spec).unwrap_err().0;
    assert_eq!(bad(r#"{"root":"HKZZ","path":"Software"}"#), ERR_INVALID_ARG);
    assert_eq!(bad(r#"{"path":"Software"}"#), ERR_INVALID_ARG);
    assert_eq!(bad(r#"{"root":"HKCU"}"#), ERR_INVALID_ARG);
    assert_eq!(bad(r#"{"root":"HKCU","path":""}"#), ERR_INVALID_ARG);
    assert_eq!(
        bad(r#"{"root":"HKCU","path":"Software\\\\"}"#),
        ERR_INVALID_ARG,
        "trailing backslash = empty segment"
    );
    assert_eq!(
        bad(r#"{"root":"HKCU","path":"Software","bogus":1}"#),
        ERR_INVALID_ARG,
        "unknown key"
    );
    assert_eq!(
        bad(r#"{"root":"HKCU","root":"HKCU","path":"Software"}"#),
        ERR_INVALID_ARG,
        "duplicate key is a parse error"
    );
    assert_eq!(bad("not json"), ERR_INVALID_ARG);
    assert_eq!(bad(r#"["HKCU"]"#), ERR_INVALID_ARG);
    // Root names round-trip.
    for r in ["HKCU", "HKLM", "HKCR", "HKU"] {
        assert_eq!(Root::parse(r).unwrap().as_str(), r);
    }
}

// -- fake backend mechanics ----------------------------------------------------

#[test]
fn fake_backend_contracts() {
    let b = FakeBackend::new();
    assert!(!b.key_exists(Root::Hkcu, "Software\\A").unwrap());
    b.create_key(Root::Hkcu, "Software\\A\\B").unwrap();
    assert!(b.key_exists(Root::Hkcu, "Software").unwrap());
    assert!(b.key_exists(Root::Hkcu, "Software\\A\\B").unwrap());
    // delete_key refuses while a subkey exists.
    assert!(b.delete_key(Root::Hkcu, "Software\\A").is_err());
    b.delete_key(Root::Hkcu, "Software\\A\\B").unwrap();
    b.delete_key(Root::Hkcu, "Software\\A").unwrap();
    assert_eq!(
        b.read_value(Root::Hkcu, "Software\\A", None).unwrap_err().0,
        ERR_NOT_FOUND
    );
}

// -- write + undo ---------------------------------------------------------------

#[test]
fn write_fresh_tree_then_undo_removes_it() {
    let b = FakeBackend::new();
    b.create_key(Root::Hkcu, "Software").unwrap();
    let result = write_with(
        &b,
        &spec_write(
            "Software\\RegKit\\Deep",
            Some("Greeting"),
            "sz",
            "\"hello\"",
        ),
    )
    .unwrap();
    assert_eq!(
        read_val(&b, "Software\\RegKit\\Deep", Some("Greeting")),
        "{\"type\":\"sz\",\"value\":\"hello\"}"
    );
    let manifest = manifest_of(&result);
    let m = json::parse(&manifest).unwrap();
    assert_eq!(m.get("version").and_then(Value::as_i64), Some(1));
    let entries = m.get("entries").and_then(Value::as_array).unwrap();
    // Created keys: RegKit and RegKit\Deep (Software existed).
    let key_removes: Vec<&Value> = entries
        .iter()
        .filter(|e| e.get("action").and_then(Value::as_str) == Some("key_remove"))
        .collect();
    assert_eq!(key_removes.len(), 2);
    // The value did not exist before: value_remove entry.
    assert!(entries
        .iter()
        .any(|e| e.get("action").and_then(Value::as_str) == Some("value_remove")));

    apply_undo_with(&b, &undo_spec(&manifest)).unwrap();
    assert!(!b.has_key(Root::Hkcu, "Software\\RegKit\\Deep"));
    assert!(!b.has_key(Root::Hkcu, "Software\\RegKit"));
    assert!(
        b.has_key(Root::Hkcu, "Software"),
        "pre-existing key must survive undo"
    );
}

#[test]
fn write_over_existing_value_undo_restores_old() {
    let b = FakeBackend::new();
    b.set_value(
        Root::Hkcu,
        "Software\\App",
        Some("Mode"),
        &RegValue::Sz("old".into()),
    )
    .unwrap();
    let result = write_with(
        &b,
        &spec_write("Software\\App", Some("Mode"), "sz", "\"new\""),
    )
    .unwrap();
    assert_eq!(
        read_val(&b, "Software\\App", Some("Mode")),
        "{\"type\":\"sz\",\"value\":\"new\"}"
    );
    let manifest = manifest_of(&result);
    assert!(manifest.contains("\"value_restore\""), "{manifest}");
    assert!(manifest.contains("\"old\""), "{manifest}");
    apply_undo_with(&b, &undo_spec(&manifest)).unwrap();
    assert_eq!(
        read_val(&b, "Software\\App", Some("Mode")),
        "{\"type\":\"sz\",\"value\":\"old\"}"
    );
}

#[test]
fn write_changes_type_and_undo_restores_type_too() {
    let b = FakeBackend::new();
    b.set_value(Root::Hkcu, "Software\\App", Some("N"), &RegValue::Dword(3))
        .unwrap();
    let result = write_with(
        &b,
        &spec_write("Software\\App", Some("N"), "sz", "\"three\""),
    )
    .unwrap();
    apply_undo_with(&b, &undo_spec(&manifest_of(&result))).unwrap();
    assert_eq!(
        read_val(&b, "Software\\App", Some("N")),
        "{\"type\":\"dword\",\"value\":3}"
    );
}

#[test]
fn write_default_value_and_read_without_name() {
    let b = FakeBackend::new();
    write_with(&b, &spec_write("Software\\App", None, "sz", "\"def\"")).unwrap();
    assert_eq!(
        read_val(&b, "Software\\App", None),
        "{\"type\":\"sz\",\"value\":\"def\"}"
    );
    // An explicit empty name reads the default value too.
    assert_eq!(
        read_val(&b, "Software\\App", Some("")),
        "{\"type\":\"sz\",\"value\":\"def\"}"
    );
}

#[test]
fn write_all_types_roundtrip_through_ops() {
    let b = FakeBackend::new();
    let cases = [
        (
            "S",
            "sz",
            "\"text\"",
            "{\"type\":\"sz\",\"value\":\"text\"}",
        ),
        (
            "E",
            "expand_sz",
            "\"%X%\"",
            "{\"type\":\"expand_sz\",\"value\":\"%X%\"}",
        ),
        ("D", "dword", "42", "{\"type\":\"dword\",\"value\":42}"),
        (
            "Q",
            "qword",
            "9007199254740993",
            "{\"type\":\"qword\",\"value\":9007199254740993}",
        ),
        (
            "M",
            "multi_sz",
            "[\"one\",\"two\"]",
            "{\"type\":\"multi_sz\",\"value\":[\"one\",\"two\"]}",
        ),
        (
            "B",
            "binary",
            "\"AAEC\"",
            "{\"type\":\"binary\",\"value\":\"AAEC\"}",
        ),
    ];
    for (name, ty, value, expect) in cases {
        write_with(&b, &spec_write("Software\\Types", Some(name), ty, value)).unwrap();
        assert_eq!(
            read_val(&b, "Software\\Types", Some(name)),
            expect,
            "{name}"
        );
    }
    let listing = list_with(&b, &spec_read("Software\\Types", None)).unwrap();
    let v = json::parse(&listing).unwrap();
    assert_eq!(v.get("values").and_then(Value::as_array).unwrap().len(), 6);
}

// -- read / list -----------------------------------------------------------------

#[test]
fn read_errors() {
    let b = FakeBackend::new();
    assert_eq!(
        read_with(&b, &spec_read("Software\\Nope", Some("X")))
            .unwrap_err()
            .0,
        ERR_NOT_FOUND,
        "missing key"
    );
    b.create_key(Root::Hkcu, "Software\\Empty").unwrap();
    assert_eq!(
        read_with(&b, &spec_read("Software\\Empty", Some("X")))
            .unwrap_err()
            .0,
        ERR_NOT_FOUND,
        "missing value"
    );
    assert_eq!(
        read_with(&b, &spec_read("Software\\Empty", None))
            .unwrap_err()
            .0,
        ERR_NOT_FOUND,
        "default value not set"
    );
    assert_eq!(
        list_with(&b, &spec_read("Software\\Nope", None))
            .unwrap_err()
            .0,
        ERR_NOT_FOUND
    );
}

#[test]
fn list_shape_includes_default_and_subkeys() {
    let b = FakeBackend::new();
    b.set_value(Root::Hkcu, "Software\\App", None, &RegValue::Sz("d".into()))
        .unwrap();
    b.set_value(
        Root::Hkcu,
        "Software\\App",
        Some("Zed"),
        &RegValue::Dword(1),
    )
    .unwrap();
    b.create_key(Root::Hkcu, "Software\\App\\Beta").unwrap();
    b.create_key(Root::Hkcu, "Software\\App\\Alpha").unwrap();
    let out = list_with(&b, &spec_read("Software\\App", None)).unwrap();
    let v = json::parse(&out).unwrap();
    let subs: Vec<&str> = v
        .get("subkeys")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap())
        .collect();
    assert_eq!(subs, vec!["Alpha", "Beta"], "subkeys sorted");
    let values = v.get("values").and_then(Value::as_array).unwrap();
    assert_eq!(values.len(), 2);
    // The default value is listed with name "".
    let def = values
        .iter()
        .find(|e| e.get("name").and_then(Value::as_str) == Some(""))
        .expect("default value listed");
    assert_eq!(def.get("type").and_then(Value::as_str), Some("sz"));
    assert_eq!(def.get("value").and_then(Value::as_str), Some("d"));
}

// -- delete + undo ---------------------------------------------------------------

#[test]
fn delete_value_then_undo_restores() {
    let b = FakeBackend::new();
    b.set_value(
        Root::Hkcu,
        "Software\\App",
        Some("Temp"),
        &RegValue::Dword(9),
    )
    .unwrap();
    let result = delete_with(
        &b,
        &format!(
            "{{\"root\":\"HKCU\",\"path\":\"{}\",\"name\":\"Temp\"}}",
            jp("Software\\App")
        ),
    )
    .unwrap();
    assert_eq!(
        read_with(&b, &spec_read("Software\\App", Some("Temp")))
            .unwrap_err()
            .0,
        ERR_NOT_FOUND
    );
    apply_undo_with(&b, &undo_spec(&manifest_of(&result))).unwrap();
    assert_eq!(
        read_val(&b, "Software\\App", Some("Temp")),
        "{\"type\":\"dword\",\"value\":9}"
    );
}

#[test]
fn delete_missing_things_is_not_found() {
    let b = FakeBackend::new();
    b.create_key(Root::Hkcu, "Software\\App").unwrap();
    assert_eq!(
        delete_with(
            &b,
            &format!(
                "{{\"root\":\"HKCU\",\"path\":\"{}\",\"name\":\"Ghost\"}}",
                jp("Software\\App")
            )
        )
        .unwrap_err()
        .0,
        ERR_NOT_FOUND
    );
    assert_eq!(
        delete_with(&b, &spec_read("Software\\Ghost", None))
            .unwrap_err()
            .0,
        ERR_NOT_FOUND
    );
}

#[test]
fn delete_key_with_subkeys_refused_without_recursive() {
    let b = FakeBackend::new();
    b.set_value(
        Root::Hkcu,
        "Software\\Tree\\Child",
        Some("V"),
        &RegValue::Sz("x".into()),
    )
    .unwrap();
    let err = delete_with(&b, &spec_read("Software\\Tree", None)).unwrap_err();
    assert_eq!(err.0, ERR_INVALID_ARG);
    assert!(err.1.contains("recursive"), "{}", err.1);
    // Nothing was deleted.
    assert!(b.has_key(Root::Hkcu, "Software\\Tree"));
    assert!(b.has_key(Root::Hkcu, "Software\\Tree\\Child"));
    assert_eq!(
        read_val(&b, "Software\\Tree\\Child", Some("V")),
        "{\"type\":\"sz\",\"value\":\"x\"}"
    );
}

#[test]
fn delete_key_with_values_only_then_undo() {
    let b = FakeBackend::new();
    b.set_value(
        Root::Hkcu,
        "Software\\Leaf",
        None,
        &RegValue::Sz("root-val".into()),
    )
    .unwrap();
    b.set_value(Root::Hkcu, "Software\\Leaf", Some("N"), &RegValue::Qword(5))
        .unwrap();
    let result = delete_with(&b, &spec_read("Software\\Leaf", None)).unwrap();
    assert!(!b.has_key(Root::Hkcu, "Software\\Leaf"));
    apply_undo_with(&b, &undo_spec(&manifest_of(&result))).unwrap();
    assert!(b.has_key(Root::Hkcu, "Software\\Leaf"));
    assert_eq!(
        read_val(&b, "Software\\Leaf", None),
        "{\"type\":\"sz\",\"value\":\"root-val\"}"
    );
    assert_eq!(
        read_val(&b, "Software\\Leaf", Some("N")),
        "{\"type\":\"qword\",\"value\":5}"
    );
}

#[test]
fn delete_recursive_subtree_then_undo_restores_all() {
    let b = FakeBackend::new();
    b.set_value(
        Root::Hkcu,
        "Software\\Tree",
        Some("Top"),
        &RegValue::Sz("t".into()),
    )
    .unwrap();
    b.set_value(Root::Hkcu, "Software\\Tree\\Mid", None, &RegValue::Dword(2))
        .unwrap();
    b.set_value(
        Root::Hkcu,
        "Software\\Tree\\Mid\\Deep",
        Some("Bin"),
        &RegValue::Binary(vec![0xde, 0xad]),
    )
    .unwrap();
    b.create_key(Root::Hkcu, "Software\\Tree\\Empty").unwrap();
    let keys_before = b.key_count();

    let spec = format!(
        "{{\"root\":\"HKCU\",\"path\":\"{}\",\"recursive\":true}}",
        jp("Software\\Tree")
    );
    let result = delete_with(&b, &spec).unwrap();
    assert!(!b.has_key(Root::Hkcu, "Software\\Tree"));
    assert!(!b.has_key(Root::Hkcu, "Software\\Tree\\Mid\\Deep"));
    assert!(
        b.has_key(Root::Hkcu, "Software"),
        "parent outside the tree survives"
    );

    apply_undo_with(&b, &undo_spec(&manifest_of(&result))).unwrap();
    assert_eq!(b.key_count(), keys_before);
    assert_eq!(
        read_val(&b, "Software\\Tree", Some("Top")),
        "{\"type\":\"sz\",\"value\":\"t\"}"
    );
    assert_eq!(
        read_val(&b, "Software\\Tree\\Mid", None),
        "{\"type\":\"dword\",\"value\":2}"
    );
    assert_eq!(
        read_val(&b, "Software\\Tree\\Mid\\Deep", Some("Bin")),
        "{\"type\":\"binary\",\"value\":\"3q0=\"}"
    );
    assert!(b.has_key(Root::Hkcu, "Software\\Tree\\Empty"));
}

// -- apply_undo validation ---------------------------------------------------------

#[test]
fn apply_undo_rejects_bad_manifests() {
    let b = FakeBackend::new();
    let bad = |spec: &str| apply_undo_with(&b, spec).unwrap_err().0;
    assert_eq!(bad(r#"{}"#), ERR_INVALID_ARG, "missing manifest member");
    assert_eq!(bad(&undo_spec("not json")), ERR_INVALID_ARG);
    assert_eq!(
        bad(&undo_spec(r#"{"version":2,"root":"HKCU","entries":[]}"#)),
        ERR_INVALID_ARG,
        "version must be 1"
    );
    assert_eq!(
        bad(&undo_spec(r#"{"version":1,"root":"HKCU"}"#)),
        ERR_INVALID_ARG,
        "entries required"
    );
    assert_eq!(
        bad(&undo_spec(
            r#"{"version":1,"root":"HKCU","entries":[{"action":"explode","path":"Software"}]}"#
        )),
        ERR_INVALID_ARG,
        "unknown action"
    );
    // An empty entries list is a valid no-op undo.
    apply_undo_with(
        &b,
        &undo_spec(r#"{"version":1,"kind":"regkit-undo","root":"HKLM","entries":[]}"#),
    )
    .unwrap();
}

#[test]
fn undo_is_tolerant_of_already_restored_state() {
    // Applying the same manifest twice: the second pass
    // finds the value already gone / key already gone and
    // treats those steps as done (FileOpsKit's rule).
    let b = FakeBackend::new();
    b.create_key(Root::Hkcu, "Software").unwrap();
    let result = write_with(&b, &spec_write("Software\\Once", Some("V"), "sz", "\"1\"")).unwrap();
    let manifest = manifest_of(&result);
    apply_undo_with(&b, &undo_spec(&manifest)).unwrap();
    apply_undo_with(&b, &undo_spec(&manifest)).unwrap();
    assert!(!b.has_key(Root::Hkcu, "Software\\Once"));
}

#[test]
fn escapes_json() {
    assert_eq!(json_escape("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
}
