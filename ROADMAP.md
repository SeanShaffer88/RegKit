# RegKit Roadmap

## v0.1.0 (current)
Registry read/list/write/delete behind a C ABI: JSON specs naming one of the four roots and a backslash key path; the six everyday value types (sz, expand_sz, dword, qword, multi_sz, binary as base64); the default value addressed by omitting the name; every write and delete returning a self-contained undo manifest (prior value, or value/key-created markers) that `rg_apply_undo` plays back in reverse; recursive key delete only on explicit `"recursive": true`, with a full subtree snapshot behind its manifest. All operation logic runs against a backend trait — advapi32 on Windows, an in-memory fake in the host tests.

## Next, in order
1. **Field verification**: run the Windows CI smoke test green, then a field test inside a real app (InstallScope's registry reads are the natural first consumer), and raise the PARITY/QUALITY platform scores on that evidence.
2. **Registry change watching**: a `rg_watch` companion (RegNotifyChangeKeyValue) feeding the same spec shapes, so hosts stop polling for Run-key / association changes.
3. **Key metadata in listings**: last-write time and a value count summary in `rg_list`, for tools that show "when did this change".
4. **Transactions**: optional KTM transaction wrapping (RegCreateKeyTransacted / RegSetValueEx transacted) so a multi-value write can commit or roll back atomically instead of relying on the manifest.
5. **Remaining value types**: reg_none and dword_big_endian read support (display-only; writes stay with the six round-trippable types).

## Explicitly not planned
- Remote registry (RegConnectRegistry): a different trust and latency world; hosts that need it should say so as its own kit.
- A `.reg` file importer/exporter: file formats for registry data belong with the document/data kits; RegKit speaks JSON specs.
- Permission/ACL editing (registry key security descriptors): high-risk surface that deserves its own reviewed design, not a spec flag.
