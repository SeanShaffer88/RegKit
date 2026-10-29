/* RegKit.dll - Windows registry access behind a small C ABI.
 *
 * Read, list, write and delete registry values and keys, where
 * every write and delete returns an UNDO MANIFEST: a JSON
 * record of the prior state (the old value, or the fact that
 * a value/key did not exist) that rg_apply_undo() plays back
 * to restore exactly what was there before.
 *
 * Strings: all strings are UTF-8, NUL-terminated. Key paths
 * are backslash-separated ("Software\MyApp") with no empty
 * segments.
 *
 * Ownership: every char* result is caller-owned and must be
 * freed with rg_free_string(). rg_version() and
 * rg_last_error() return borrowed strings - do NOT free them.
 *
 * Errors: a NULL return (or a nonzero code from
 * rg_apply_undo) means failure; rg_last_error() /
 * rg_last_error_code() describe it on the calling thread.
 *
 * Roots: "HKCU", "HKLM", "HKCR", "HKU" - exactly those four.
 *
 * Value types and their JSON representation:
 *   "sz"        string            {"type":"sz","value":"text"}
 *   "expand_sz" string            (may contain %VAR% references)
 *   "dword"     number 0..4294967295
 *   "qword"     number 0..9223372036854775807 (see Limitations)
 *   "multi_sz"  array of strings
 *   "binary"    base64 string (standard alphabet, padded)
 *
 * The default value: a key's unnamed ("(Default)") value is
 * addressed by OMITTING "name" from a spec, or by passing
 * "name":"". In rg_list output it appears with "name":"".
 *
 * Platform: the registry backend is Windows-only. On any
 * other platform every operation fails cleanly with
 * RG_ERR_NOT_SUPPORTED.
 */
#ifndef REGKIT_H
#define REGKIT_H
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
#if defined(_WIN32) && defined(RG_BUILD)
#define RG_API __declspec(dllexport)
#elif defined(_WIN32)
#define RG_API __declspec(dllimport)
#else
#define RG_API
#endif

#define RG_OK 0
#define RG_ERR_INVALID_ARG 1
#define RG_ERR_NOT_SUPPORTED 2
#define RG_ERR_NOT_FOUND 3
#define RG_ERR_ACCESS_DENIED 4
#define RG_ERR_PLATFORM 5
#define RG_ERR_INTERNAL 6

/* Library version, e.g. "0.1.0". Borrowed string; do not free. */
RG_API const char *rg_version(void);

/* Last error message on this thread ("" when none). Borrowed. */
RG_API const char *rg_last_error(void);

/* Last RG_ERR_* code on this thread (RG_OK when none). */
RG_API int rg_last_error_code(void);

/* Free a caller-owned string returned by this library.
 * NULL is accepted and does nothing. */
RG_API void rg_free_string(char *p);

/* Read one value.
 *
 * spec_json: {"root":"HKCU","path":"Software\\MyApp",
 *             "name":"Version"}        ("name" optional)
 * With "name": that value. Without: the key's default value.
 *
 * Returns caller-owned JSON: {"type":"sz","value":...} (the
 * "value" shape depends on the type - see the table above).
 * Returns NULL + last-error when the key does not exist
 * (RG_ERR_NOT_FOUND), the value is not set
 * (RG_ERR_NOT_FOUND), or the spec is invalid
 * (RG_ERR_INVALID_ARG). A stored value of an unsupported
 * registry type (anything outside the six) fails with
 * RG_ERR_NOT_SUPPORTED rather than being guessed at. */
RG_API char *rg_read(const char *spec_json);

/* List one key's immediate contents.
 *
 * spec_json: {"root":"HKCU","path":"Software\\MyApp"}
 *
 * Returns caller-owned JSON:
 *   {"subkeys":["ChildA","ChildB"],
 *    "values":[{"name":"Version","type":"sz","value":"1.0"},
 *              {"name":"","type":"sz","value":"default..."}]}
 * ("name":"" is the default value.) Subkeys are sorted.
 * NULL + RG_ERR_NOT_FOUND when the key does not exist. */
RG_API char *rg_list(const char *spec_json);

/* Write one value, creating the key path if needed.
 *
 * spec_json: {"root":"HKCU","path":"Software\\MyApp",
 *             "name":"Version",          (optional: default value)
 *             "type":"sz","value":"1.0"}
 *
 * The prior state is captured BEFORE anything changes:
 * which ancestor keys did not exist, and the value's old
 * type+value (or that it did not exist). Returns
 * caller-owned JSON: {"undo_manifest":"<manifest JSON>"}
 * - keep the manifest string if you want the write to be
 * reversible with rg_apply_undo(). The manifest is
 * self-contained data, not a file path; storing it is the
 * caller's choice.
 *
 * Refusal: if the value being overwritten is a qword above
 * 9223372036854775807 it cannot be recorded exactly in a
 * manifest, so the write fails up front with
 * RG_ERR_PLATFORM and changes nothing. */
RG_API char *rg_write(const char *spec_json);

/* Delete one value, or one key.
 *
 * Value delete:  {"root":...,"path":...,"name":"Version"}
 * Key delete:    {"root":...,"path":...}
 *                ("recursive":true required when the key
 *                 has subkeys)
 *
 * A key with values but no subkeys deletes without
 * "recursive". A key WITH subkeys and no "recursive":true
 * fails with RG_ERR_INVALID_ARG and deletes NOTHING.
 * A recursive delete snapshots the whole subtree before
 * deleting anything.
 *
 * Returns caller-owned JSON: {"undo_manifest":"..."} with
 * the same prior-state record as rg_write: undoing a value
 * delete rewrites the value; undoing a key delete recreates
 * the key tree with all recorded values. */
RG_API char *rg_delete(const char *spec_json);

/* Apply an undo manifest from rg_write / rg_delete.
 *
 * spec_json: {"manifest":"<the undo_manifest string>"}
 *
 * Manifest format (version 1):
 *   {"version":1,"kind":"regkit-undo","root":"HKCU",
 *    "entries":[ ...undo primitives... ]}
 * Entries are applied in REVERSE order: value_restore
 * rewrites a recorded old value (recreating its key if
 * needed), value_remove deletes a value the operation
 * created, key_create ensures a deleted key exists again,
 * key_remove deletes a key the operation created. Steps
 * whose target is already in the desired state (value or
 * key already gone) count as done, so re-applying a
 * manifest is safe.
 *
 * Returns RG_OK (0) on success. On failure it returns the
 * RG_ERR_* code and sets the last error; undo stops at the
 * first failing entry and entries already applied stay
 * applied. Note the asymmetry with a file undo: this
 * restores registry state, and rg_delete's refusal rules
 * mean a key_remove step fails if the key has gained
 * subkeys since the manifest was written. */
RG_API int rg_apply_undo(const char *spec_json);

#ifdef __cplusplus
}
#endif
#endif
