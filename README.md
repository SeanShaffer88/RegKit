# RegKit

## TL;DR

RegKit is a small DLL for reading and writing the Windows registry — the settings database every Windows program touches — without the usual danger. You describe each job in plain JSON: read a value, list a key, write a value, delete a value or a key. Every write and delete hands you back an "undo manifest" — a short record of exactly what was there before — and one more call plays that record back to put everything back the way it was. It is like a careful worker who photographs the room before moving any furniture.

```c
#include "regkit.h"
#include <stdio.h>

int main(void) {
    /* Write a value; keep the undo manifest. */
    char *w = rg_write("{\"root\":\"HKCU\",\"path\":\"Software\\\\MyApp\","
                       "\"name\":\"Version\",\"type\":\"sz\",\"value\":\"1.0\"}");
    printf("%s\n", w);   /* {"undo_manifest":"{...}"} */

    /* Read it back. */
    char *r = rg_read("{\"root\":\"HKCU\",\"path\":\"Software\\\\MyApp\","
                      "\"name\":\"Version\"}");
    printf("%s\n", r);   /* {"type":"sz","value":"1.0"} */

    /* Roll the write back (pass the manifest from w). */
    /* rg_apply_undo("{\"manifest\":\"...\"}"); */

    rg_free_string(w);
    rg_free_string(r);
    return 0;
}
```

## Key features

- **Four calls cover the registry.** `rg_read` (one value, or the key's default value when no name is given), `rg_list` (a key's subkeys and values), `rg_write` (creates the key path if needed), `rg_delete` (a value, or a key).
- **Undo manifests, in the FileOpsKit discipline.** The prior state is captured *before* anything changes: which ancestor keys did not exist, the old value's type and contents (or that there was no value). `rg_apply_undo` applies the manifest in reverse: it rewrites old values, removes values the write created, recreates keys a delete removed (with their values), and removes keys a write created. Re-applying a manifest is safe — steps already in the desired state count as done.
- **Recursive delete is opt-in and honest.** Deleting a key that has subkeys is refused unless the spec says `"recursive": true`; the refusal deletes nothing. A recursive delete snapshots the whole subtree first, so its undo restores the whole subtree.
- **Six value types, represented exactly.** `sz` and `expand_sz` as strings, `dword` and `qword` as numbers, `multi_sz` as an array of strings, `binary` as a base64 string. A stored value of any other registry type is reported as an error (RG_ERR_NOT_SUPPORTED), never guessed at or mangled.
- **The default value is first-class.** Omit `"name"` (or pass `""`) to address a key's `(Default)` value; listings show it with `"name": ""`.
- **Backend trait, testable anywhere.** All logic runs against a backend trait; the real backend talks to advapi32 on Windows, and the test suite runs the full operation set against an in-memory fake, so manifest build/apply logic is verified on any host. Off Windows, every call fails cleanly with RG_ERR_NOT_SUPPORTED.
- **Zero dependencies.** Hand-rolled JSON (duplicate keys in one object are a parse error, by design), hand-declared advapi32 bindings, small C ABI.

## The API in one screen

| Call | What it does |
|---|---|
| `rg_read(spec_json)` | Read one value → `{"type":...,"value":...}` |
| `rg_list(spec_json)` | List a key → `{"subkeys":[...],"values":[{"name","type","value"}]}` |
| `rg_write(spec_json)` | Create key path if needed + set value → `{"undo_manifest":"..."}` |
| `rg_delete(spec_json)` | Delete a value, or a key (`"recursive": true` for a tree) → `{"undo_manifest":"..."}` |
| `rg_apply_undo(spec_json)` | Play a manifest back → `RG_OK` or an `RG_ERR_*` code |
| `rg_version` / `rg_last_error` / `rg_last_error_code` / `rg_free_string` | Version, thread-local error, string ownership |

Specs name a root (`HKCU`, `HKLM`, `HKCR`, `HKU`) and a backslash key path. Hard failures return NULL (or the error code) and set the thread-local error.

## Build

```sh
cargo build --release   # reg_kit.dll / .so / .a + include/regkit.h
cargo test              # full test suite
```

Rust 1.80+, edition 2021, no dependencies.

## Limitations, honestly

- **Windows runtime is Unverified from this repo's local gate.** The operations are fully tested against the in-memory fake on the development host, and a Windows-only smoke test (a real HKCU roundtrip in a throwaway key) is in the suite for CI to run — but until that runs on a real Windows machine, the advapi32 backend's behaviour is implemented-and-checked, not field-proven. See `docs/PARITY.md`.
- **Qword range.** JSON numbers in a spec are limited to 9223372036854775807 for `qword` writes. A stored qword above that reads back fine, but *overwriting or deleting* one is refused (RG_ERR_PLATFORM), because its old value could not be recorded exactly in an undo manifest.
- **Undo is a restore, not a force.** If a key a write created has since gained subkeys, the manifest's key-removal step fails and undo reports it; entries already applied stay applied. Undo restores registry state — it does not track changes made by other programs after the manifest was written.
- **No registry watching, no remote registry, no transactions.** Reads and writes are individual advapi32 calls; there is no RegNotifyChangeKeyValue, no RegConnectRegistry, and no KTM transaction wrapping (yet — see ROADMAP).
- **Access is the caller's.** HKLM writes need an elevated process; failures come back as RG_ERR_ACCESS_DENIED. The DLL never elevates or works around permissions.
- **Synchronous.** Calls do the work on the calling thread.

## Tests

29 host tests: base64 vectors and rejection cases, all six types' JSON conversions plus bad-shape rejections, spec validation, the fake backend's contracts, write/undo for fresh trees, overwritten values and changed types, default-value handling, read/list shapes and not-found errors, value delete + undo, the non-recursive refusal (with nothing deleted), key delete + undo, recursive subtree delete + full restore, manifest validation, and double-apply tolerance. Plus the Windows-only real-HKCU smoke test in `src/platform.rs`, which runs under Windows CI.

## License

MIT. Part of the WinDevKit family — see `AGENTS.md` for build discipline and `ROADMAP.md` for what is next.
