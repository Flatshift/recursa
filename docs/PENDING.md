# Pending fixes

Known problems that can't be fixed in this repository yet, because the fix has to come from upstream first. Every CI
run checks each one with `tools/pending.py`, writes the result to the run's summary, and raises a warning when a fix
can be applied. Remove an entry here and in `tools/pending.py` once its fix is in.

## quick-xml denial of service (RUSTSEC-2026-0194, RUSTSEC-2026-0195)

- **What:** quick-xml 0.39 has quadratic run time on duplicate attribute checks and unbounded memory for namespace
  declarations. Both are fixed in 0.41.
- **Where it comes from:** `rpki` 0.19.3, through `routinator` 0.15.2, which parses RRDP XML from RPKI repositories.
  Both are their newest releases.
- **Exposure:** only with local route-origin validation turned on in Settings (`rpki-local`), and only to a hostile
  RPKI publication point. Denial of service, no code execution.
- **Meanwhile:** `.cargo/audit.toml` ignores the two advisories so that `cargo audit` in CI fails on anything new.
- **Fixed when:** `cargo update` moves quick-xml to 0.41 or later, or a newer `routinator` does. Then remove the two
  ignores.

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

## Forged Kiss-o'-Death for plain NTP

- **What:** an attacker on the network path can forge a plain NTP reply, a Kiss-o'-Death included, because plain NTP
  has no authentication. A forged DENY makes Recursa leave that server alone for a week.
- **Fixed when:** not fixable for plain NTP. NTS servers are protected: unverified replies are discarded. Not checked
  in CI.
