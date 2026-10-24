# Recursa

A recursive DNS resolver benchmark that measures from **your** computer. It compares public resolvers (Cloudflare, Google,
Quad9, OpenDNS, AdGuard, Control D, NextDNS, Mullvad, DNS4EU, …), your system or ISP resolver, and any custom server. It
covers latency, what each resolver actually does, how well it maps you onto CDNs, and how reliable it is.

It comes two ways, built from the same engine and interface:

* **The Recursa app** for macOS and Windows. It keeps a menu-bar (tray) item, so monitoring continues with the window
  closed, and it can open at login.
* **`recursa`**, a single command-line binary. It runs benchmarks in the terminal, or serves the same interface to your
  browser on `127.0.0.1`.

## Install

Download from the [Releases page](https://github.com/flatshift/recursa/releases). The builds are not yet signed with
a developer certificate, so each system warns once:

* **macOS** (`Recursa_<version>_universal.dmg`, Apple silicon and Intel): open the DMG and drag Recursa to Applications.
  The first time, right-click Recursa.app and choose **Open**. If macOS says the app is damaged, clear the download
  quarantine mark and open it again:

  ```bash
  xattr -dr com.apple.quarantine /Applications/Recursa.app
  ```

* **Windows** (`Recursa_<version>_x64-setup.exe`, or `_arm64-setup.exe` for Windows on ARM): run the installer; it
  installs for the current user and needs no administrator rights. When SmartScreen appears, choose **More info**, then
  **Run anyway**.

Or build it yourself (Rust, Node 22.12+):

```bash
./build.sh            # builds the UI, then the command-line binary
./build.sh app        # also bundles Recursa.app and a DMG (target/release/bundle/)
./target/release/recursa
```

## What it measures

| Area | How |
|---|---|
| **Latency** | Cached, uncached (random names under unsigned wildcard zones, so the resolver must recurse) and TLD (random `.com` NXDOMAIN) lookups over UDP, TCP, DoT and DoH. Every resolver gets one query per round in shuffled order. |
| **Timing precision** | UDP receive stamps come from the kernel (`SO_TIMESTAMP_MONOTONIC` on macOS, `SO_TIMESTAMPNS` on Linux, `SIO_TIMESTAMPING` on Windows 10 2004 and later) on dedicated, prioritized measurement threads (user-interactive QoS on macOS, time-critical priority on Windows). Queries are encoded before the send stamp. A loopback self-calibration publishes the instrument's own floor (≈ 45–50 µs on an M4 Max). |
| **Interception** | Two independent signals. A TCP handshake to each resolver gives the true path RTT, and no genuine answer can beat it. The IP TTL of every UDP answer gives the router hops it crossed, and a public resolver 0–1 hops away is a device on your LAN (DNAT rules, or AP features such as UniFi DNS Assistance). Flagged targets are measured but not ranked, and their probes switch to encrypted DNS. A standalone check runs at startup with **Re-check**, or run `recursa check`. |
| **Capabilities** | DNSSEC validation (and AD bit), EDNS(0) and version negotiation, ECS, QNAME minimisation, NSID and anycast site, DNS Cookies, Extended DNS Errors, NXDOMAIN rewriting, rebinding protection, TCP/DoT/DoH/IPv6 reachability. Every probe uses random labels so no cache can answer it, and each check keeps its evidence. |
| **Blocking** | Ad/tracker hosts, and the malware, phishing and adult-content test domains that Cloudflare, Quad9 and Cisco OpenDNS publish for testing their blocking. Results show the block style (0.0.0.0, NXDOMAIN, block page, EDE 15–18), and **Not tested** for a check the run left off. For a closer look, Check live feeds on a resolver's page tests one resolver against live URLhaus and OpenPhish hosts. |
| **CDN mapping** | Each resolver resolves DNS-steered CDN hostnames (Akamai, Microsoft, Apple, CloudFront, Google, Fastly, Meta; Cloudflare as an anycast control). Recursa times TCP handshakes to the edges returned and reports each resolver's penalty against the best edge seen. It labels why: on-net, ECS with your subnet, ECS with a substitute subnet, or resolver location. Edges inside your own ISP are marked. |
| **Routing security** | RPKI ROA validity of the resolver's prefix, checked on this computer if you turn it on in Settings (a built-in RRDP validator; origin AS and prefix from Team Cymru), and the operator network's route-origin validation (RoVista). |
| **Resolver page** | A read-only look at one resolver, from the Inspector or its menu, with a button for each check. **Sample 18 queries** sends 18 ordinary queries to give the spread of its latency, cold versus warm cache, how long answers stay cached, and the fields of one response (rcode, flags, EDNS, DNSSEC, EDE, NSID). A diagnostic, not a load test: the query count is fixed. **Check behaviour**, when you start it, tests large answers and TCP fallback, TTL caps, negative caching, DNSSEC with known-broken test names, how many caches sit behind the address, and what each transport's connection setup costs. **Check live feeds**, opt-in, asks it for live malware (URLhaus) and phishing (OpenPhish) hosts to gauge how well it blocks; a network's IDS/IPS (a UniFi gateway, Azure, a corporate firewall) may flag those lookups. **Live names go over DoT/DoH** except to your own resolver on the local network (a Pi-hole, your router), or when you allow plain DNS. |
| **Link check** | While a run measures, Recursa pings your default gateway every 100 ms and times the replies with the same kernel stamps. If the link stalls (a Wi-Fi scan, a busy channel), the run says its scores are inflated, and on macOS names the likely cause. The scores are not changed. |
| **Reliability** | Live operator status (Cloudflare, Quad9, AdGuard, Cisco Umbrella/OpenDNS) judged for your country, a sourced incident history, and optional background monitoring that tests each resolver's primary and secondary server and builds an uptime record from your network. |
| **Time (NTP)** | How accurate and how fast about 60 public time servers are from your network: offset, an error bound, latency, jitter, stratum and source, and each server's distance from the consensus of all of them (which is also how far your clock is off). Queries go out eight seconds apart, a server is left alone for ten minutes after it is asked, and a Kiss-o'-Death is obeyed and remembered; a Features view sets leap-second handling, anycast, policy, IPv6 and a live NTS key-exchange check side by side. |
| **NTP over time** | Optional background monitoring of up to eight time servers, one light query per server every 15 minutes or more while Recursa runs, kept for weeks: each server's uptime, offset and delay, and how your own clock drifts. |

Results are labeled by provenance: **●** measured from your network, **○** published by the operator, **–** unknown.

## Command line

```bash
recursa                                   # open the interface (default port 7353)
recursa --no-open --port 8080             # serve without opening a browser
recursa run --samples 30                  # benchmark the default set in the terminal
recursa run --resolvers quad9,cloudflare,google --transports udp,dot,doh
recursa run --json > result.json          # full result as JSON
recursa run --cached-names example.com,wikipedia.org   # your own names for the Cached test
recursa run --cached-names-file names.txt # or a file of them (one per line)
recursa check                             # is port 53 direct, or answered on your LAN?
recursa ntp --samples 4                   # how accurate and fast are the default time servers?
recursa ntp --servers time.cloudflare.com --no-nts   # chosen servers, skipping the NTS check
recursa ntp --all --format csv            # every catalog server; --format is table, json or csv
recursa ntp --plugin                      # one-line monitoring check, exit 0 to 3 (-w, -c set the limits)
recursa calibrate                         # measure the timing floor
recursa list                              # resolver ids
```

## Build

Requirements: Rust 1.85+ (edition 2024) and Node 22.12+.

```bash
cd web && npm install && npm run build && cd ..
cargo build --release                                            # the CLI
cd crates/desktop && ../../web/node_modules/.bin/tauri build     # the app (Tauri 2), ad-hoc signed
```

The UI in `web/dist` is embedded into both at compile time. For UI work, run `recursa --no-open` and `npm run dev` in
`web/`, then open the link Recursa printed with port 5173 in place of 7353. Vite proxies `/api` to port 7353.

To check the desktop bridge without clicking, run a debug build in smoke mode. It boots the real UI, follows a short run
live, checks the timing floor with the window open and a menu command, then exits 0 or 1:

```bash
cargo build -p recursa-desktop --features custom-protocol
RECURSA_SMOKE=1 target/debug/recursa-desktop --db /tmp/smoke.db
```

## Architecture

```
crates/engine/src/   the engine (library), shared by both front ends
  app.rs         AppState and the API: every REST route, for HTTP and IPC alike
  dns.rs         wire-format encoder/decoder (OPT, NSID, ECS, Cookies, EDE)
  clock.rs       one monotonic clock domain shared with kernel stamps
  net.rs         UDP (kernel stamps), TCP, DoT (rustls), DoH (HTTP/2) transports
  bench.rs       run orchestration: setup, warm-up, interleaved phases, path check, probes, CDN, intel
  probes.rs      capability and blocking probes with evidence
  catalog.rs     resolver catalog: curated, system and custom resolvers in one shape (data/catalog.json)
  names.rs       names for the Cached test: checking, importing and parsing lists
  stats.rs       order statistics (quantiles) over round-trip samples
  resolve.rs     plain DNS questions through the system resolvers, with the engine's own DNS code
  forwarder.rs   spots a dnsmasq forwarder (a Pi-hole, a router's DNS proxy) so it is a baseline, not a pick
  deepdive.rs    Deep Dive: a short read-only look at one resolver (latency spread, cache behaviour, response fields)
  link.rs        link check: pings the gateway during a run and classifies stalls
  regions.rs     where an operator's status report applies, so a report about another region doesn't count here
  cdn.rs         CDN mapping: resolution, edge timing, penalty scoring
  endpoints.rs   per-transport endpoints, DoH/DoT bootstrap that bypasses the system resolver
  intercept.rs   standalone port-53 interception check (timing + TTL hops, and what answered)
  ntp.rs         NTP client, NTS key-exchange check, consensus (data/ntp.json is the server catalog)
  ntp_monitor.rs background NTP monitoring: uptime, offset and delay history for a few servers
  ntp_report.rs  an NTP run as JSON and CSV with a schema version, and the Nagios-style check
  calibrate.rs   loopback self-calibration
  intel.rs       Team Cymru lookups, RoVista, status feeds, threat feeds (cached; never during measurement)
  cymru.rs       Team Cymru DNS: address to origin AS, prefix and AS name
  rpki/          opt-in local RPKI validator (Routinator as a library, RRDP only), compact route-origin table, RFC 6811 lookup (Cargo feature `rpki-local`)
  monitor.rs     background monitoring and history aggregation
  store.rs       SQLite: runs, monitoring, cache, settings, custom resolvers
  lock.rs        cross-process locks: one measurement at a time per machine, one monitor per database
  sysdns.rs      system resolvers, gateway and link (scutil/route on macOS, IP Helper on Windows, resolv.conf/ip on Linux)
crates/cli/src/      the `recursa` binary
  main.rs        CLI commands and server start-up
  server.rs      HTTP transport: embedded UI, API, SSE, export; Host/Origin/token guards
crates/desktop/src/  the desktop app (Tauri 2)
  ipc.rs         IPC transport: API calls, live run channels, native save and confirm dialogs
  shell.rs       window, app menu, menu-bar item, launch at login
  chrome.rs      keeps the page clear of the macOS title bar and full-screen menu bar
  smoke.rs       debug-only end-to-end check (RECURSA_SMOKE=1)
data/
  catalog.json   curated resolvers and their published claims
  incidents.json sourced incident history
  ntp.json       curated NTP servers and what their operators publish
tools/           notices.py (third-party notices), icons.mjs (app icons), pending.py (checks docs/PENDING.md, the fixes we wait on upstream); dev helper: gen-monitor.py (synthetic monitoring history)
web/             Svelte 5 + Vite + TypeScript interface; lib/api.ts picks HTTP or IPC at runtime
```

### Local security

The app has no network listener: its window talks to the engine over in-process IPC. In browser mode, the server binds
to loopback only. Any request with a Host header other than `127.0.0.1`, `localhost` or `[::1]` on its
port is refused, which defeats DNS-rebinding. API calls need a session token, and state-changing calls must come from
the app's own Origin. No token is ever in the served page or a launch link, so another user on the same computer can't
read one from the port or from a browser's command line. Recursa opens one launch link and prints another for a second
browser. Each carries a one-time code (`#code=`) that the page trades for its own session. A link opened a second time
ends the session it started, and Recursa prints a new link: if someone else used the code first, their session ends
too. The opened link stops working after two minutes if no page used it.

### Data

A SQLite database lives in the OS application-data directory (`~/Library/Application Support/Recursa/recursa.db` on
macOS), shared by the app and the CLI. Use `--db` to put it somewhere else. On macOS and Linux the folders Recursa
creates for it are private to your user (0700), and the default folder is made private if an older version left it
open. Only one process measures at a time: a run waits briefly for a monitoring cycle in another process and refuses
if another run is in progress, and only one process records monitoring for a given database.

## Limits

* DoQ, DNSCrypt and ODoH are shown from operator documentation only; they aren't measured.
* On Windows the stack stamps a packet where the adapter hands it up, so stamps need an adapter whose driver has
  software timestamping turned on and Windows 10 2004 or later. To turn it on, open the adapter's properties in
  Device Manager, choose the Advanced tab, set **Software Timestamp** to **RxAll** (not every driver offers it)
  and restart Recursa. Answers without a stamp are timed in userspace, loopback answers always are, and the
  Method page says which applied and how to change it. The Windows link check times each echo in userspace, and
  reports only how late its own ticks ran, not loopback echoes or interface counters.
* Background monitoring runs while Recursa is running; it isn't installed as a system service.

## License

Copyright (C) 2026 Brian Morisette

This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public
License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later
version.

This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied
warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.

You should have received a copy of the GNU General Public License along with this program (the [LICENSE](LICENSE) file).
If not, see <https://www.gnu.org/licenses/>.

Some files keep their own licenses: the Recursive font in `web/src/fonts/` is under the SIL Open Font License 1.1
([OFL.txt](web/src/fonts/OFL.txt)). The licenses of the libraries built into the app and the CLI are collected in
`web/src/legal/third-party-notices.txt` (regenerate with `python3 -I tools/notices.py`), which ships in both and opens
from Settings › About.

The GPL covers the code. It grants no rights to the Recursa name or the Recursa icon: see
[TRADEMARKS.md](TRADEMARKS.md). Contributions are welcome under the terms in [CONTRIBUTING.md](CONTRIBUTING.md).
