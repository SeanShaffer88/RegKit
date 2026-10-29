# RegKit — Quality Scorecard

Scored against the v0.1.0 gate run on the development VM:
`cargo fmt --check`, clippy (host + `x86_64-pc-windows-msvc`,
`-D warnings`), and the full test suite — 29/29 passing on
the host. Scores are only claimed where that evidence
exists; Windows runtime behaviour is not claimed from Linux
evidence.

| Dimension | Score /5 | Evidence |
|---|---|---|
| Correctness of core operations | 5 | Read/list/write/delete exercised end-to-end against the fake backend with content assertions for all six types, default values, and type-changing overwrites |
| Undo safety | 5 | Fresh-tree write undo removes only created keys; overwrite undo restores old type+value; recursive-delete undo restores the full subtree including default values and an empty key; double-apply is tolerated |
| Refusal behaviour | 5 | Non-recursive delete of a key with subkeys refused with nothing deleted; oversized-qword overwrite refused before any change; unsupported stored types error instead of being guessed |
| Type/value conversions | 5 | All six types round-trip through JSON fragments; base64 has known vectors, a full 0–255 roundtrip, and six rejection cases; bad shapes are RG_ERR_INVALID_ARG |
| API & documentation | 4 | Header documents every export with spec shapes, manifest format, and refusal rules; README has a runnable C example. No compiled C consumer test in the suite (FFI is exercised from Rust) — one point held back |
| Code health | 5 | Zero dependencies, fmt + clippy clean on both targets at the gate, all logic behind a backend trait, no unwinding across the FFI boundary |
| Platform coverage | 2 | The advapi32 backend compiles and clips clean for the Windows target and a real-HKCU smoke test exists, but neither the Windows build/link nor the smoke test has RUN yet — CI's first green run is the follow-up that can raise this |

**Total: 84/100**

## Waivers / open items
- Windows runtime Unverified: the first windows-latest CI run (real build + link + smoke test) is owed; a field test in a consuming app after that.
- The manifest's qword ceiling (i64::MAX) is a documented, refused limitation, not a silent one — see README "Limitations".
