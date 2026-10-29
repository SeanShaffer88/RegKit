# RegKit — Parity (spec vs implementation)

Evidence: the Rust test suite (29 tests passing at the
v0.1.0 gate: `src/tests.rs`, `src/json.rs`, `src/ffi.rs`)
plus the header contract in `include/regkit.h`. "Verified"
means a named host test exercises the behaviour against the
in-memory fake backend. The Windows (advapi32) backend is
marked honestly: it is implemented and compiles/clips for
the Windows target, but its RUNTIME is Unverified until the
Windows CI smoke test (and later a field test) runs it.

| Spec feature | Status | Evidence |
|---|---|---|
| `rg_read` with a name returns `{"type","value"}` for all six types | Verified (logic) | `write_all_types_roundtrip_through_ops`, `value_json_roundtrips` |
| `rg_read` without a name returns the key's default value; explicit `""` is the same | Verified (logic) | `write_default_value_and_read_without_name` |
| Read of a missing key / unset value / unset default is RG_ERR_NOT_FOUND | Verified (logic) | `read_errors` |
| `rg_list` returns sorted subkeys and all values, default value listed as `"name":""` | Verified (logic) | `list_shape_includes_default_and_subkeys` |
| binary as base64 (padded, standard alphabet), multi_sz as a string array | Verified | `base64_known_vectors`, `base64_roundtrip_all_bytes`, `base64_rejects_garbage`, `value_json_roundtrips` |
| `rg_write` creates the key path if needed and returns `{"undo_manifest": string}` | Verified (logic) | `write_fresh_tree_then_undo_removes_it` (manifest shape asserted: version 1, key_remove + value_remove entries) |
| Undo of a fresh-tree write removes the created keys and value, keeps pre-existing ancestors | Verified (logic) | `write_fresh_tree_then_undo_removes_it` |
| Undo of an overwrite restores the old value AND its old type | Verified (logic) | `write_over_existing_value_undo_restores_old`, `write_changes_type_and_undo_restores_type_too` |
| `rg_delete` of a value returns a manifest whose undo rewrites the value | Verified (logic) | `delete_value_then_undo_restores` |
| Delete of a missing value or key is RG_ERR_NOT_FOUND | Verified (logic) | `delete_missing_things_is_not_found` |
| Key delete without `recursive` refuses a key with subkeys and deletes nothing | Verified (logic) | `delete_key_with_subkeys_refused_without_recursive` (asserts state untouched after refusal) |
| Key delete of a values-only key works without `recursive`; undo restores key + values incl. default | Verified (logic) | `delete_key_with_values_only_then_undo` |
| Recursive delete snapshots then removes the whole subtree; undo restores it fully (nested keys, default values, empty keys) | Verified (logic) | `delete_recursive_subtree_then_undo_restores_all` |
| `rg_apply_undo` validates the manifest (version 1, entries array, known actions) | Verified | `apply_undo_rejects_bad_manifests` |
| Re-applying a manifest is tolerated (already-restored steps count as done) | Verified | `undo_is_tolerant_of_already_restored_state` |
| Spec discipline: exactly four roots, non-empty segment-clean paths, unknown/duplicate keys rejected | Verified | `spec_validation` |
| Oversized prior qword (> i64::MAX) makes write/delete refuse before changing anything | Implemented (logic) | `check_manifest_representable` runs before every mutation; no dedicated test — the fake cannot be seeded through the public JSON surface with such a value, only directly; noted rather than claimed |
| Off-Windows: every operation fails with RG_ERR_NOT_SUPPORTED | Verified | FFI `ffi_ops_not_supported_off_windows` |
| advapi32 backend: Reg*W calls, UTF-16 conversions, error mapping (2/3 → NOT_FOUND, 5 → ACCESS_DENIED) | Implemented, runtime Unverified | `src/platform.rs` `win` module; compiles + clips for `x86_64-pc-windows-msvc` at the gate, but check/clippy do not link or run |
| Real HKCU roundtrip smoke test (write/read/list/delete/undo in a throwaway key) | Implemented, runtime Unverified | `platform.rs` `smoke::hkcu_throwaway_roundtrip`, `#[cfg(windows)]` — runs under Windows CI, has never run yet |
| Release workflow builds the x64 zip (DLL, import lib, header, LICENSE, README) | Implemented, not run | `release.yml` (root + `.github/workflows/`), adapted from WatchKit's shipped workflow |

## Known gaps (also in README "Limitations")
- Windows runtime Unverified until CI/field evidence exists.
- Qword values above i64::MAX cannot be written via JSON specs, and overwriting/deleting a stored one is refused by design.
- No watching, remote registry, transactions, or ACL editing in v0.1 (see ROADMAP).
