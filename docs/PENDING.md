# Pending fixes

Known problems that can't be fixed in this repository yet, because the fix has to come from upstream first. Every CI
run checks each one with `tools/pending.py`, writes the result to the run's summary, and raises a warning when a fix
can be applied. Remove an entry here and in `tools/pending.py` once its fix is in.

## Unmaintained or unsound crates under Tauri

- **What:** RustSec warnings, not vulnerabilities: `paste` (RUSTSEC-2024-0436), `proc-macro-error`
  (RUSTSEC-2024-0370), `rustls-pemfile` (RUSTSEC-2025-0134), `zvariant`, `zvariant_derive` and `zbus_names`
  (RUSTSEC-2026-0370, -0371, -0372) are unmaintained; `glib` 0.18 is unsound (RUSTSEC-2024-0429).
- **Where they come from:** Tauri and its Linux and build-time dependencies. Recursa calls none of them directly.
- **Fixed when:** a Tauri update drops them from `Cargo.lock`.

## TypeScript 7

- **What:** the interface stays on TypeScript 6.
- **Why:** svelte-check's peer range ends at TypeScript 6.
- **Fixed when:** a svelte-check release accepts TypeScript 7. Then update both together.
