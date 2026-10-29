# Changelog

Notable changes to Recursa, newest first. Versions follow [Semantic Versioning](https://semver.org/).

## Unreleased

- **Network page (beta).** Which site each of six CDNs serves this network from, read from the CDN's own headers, with the
  round trip against the fibre floor and whose network the edge sits in. An "Over time" view probes the two routers
  either side of your ISP's border towards each CDN in the background and says, after a day, whether the evening
  slowdown sits on the link between them or before it.
- **Route-origin filtering is tested live.** Each resolver is asked for a name served only from an RPKI-invalid route, over IPv4 and IPv6, through Check My DNS by DNS-OARC. This replaces the RoVista operator score, which did not cover every resolver.

## 0.4.3 (2026-10-26)

- **Installer layout.** The macOS disk image opens with Recursa on the left, Applications on the right and an arrow
  between them.
- **CDN notes collapsed.** The notes under the CDN results are one line that expands on click.

## 0.4.2 (2026-10-26)

- **Signed macOS app.** Recursa.app and the DMG are Developer ID signed and notarized, so macOS opens them without
  the right-click workaround.

## 0.4.1 (2026-10-25)

- **Inspector views.** A resolver's Inspector holds its six views as tabs, with each check in its own view; the
  separate resolver page is gone.
- **Batch checks.** Select resolvers in the Resolvers list to run a check on each in turn, with the results as new
  columns.
- **Hardening.** Run CSV exports defuse cells that start like a spreadsheet formula, and the launch link Recursa
  opens no longer appears on the opener's command line.
- **Leaner runs.** A run checks its helper resolvers once rather than once per test, and the CLI states the current
  score weights.
- **Path check.** A run that tests several transports now shows the path check while it runs.
- **Live threat test.** An optional last phase looks up the same live malware and phishing hosts through every selected
  resolver; Best protection ranks on it, or shows the protections each resolver holds.
- **Live threat feeds.** Malware hosts come from URLhaus, through the malware-filter mirror, and ThreatView, so Check
  live feeds downloads them again.

## 0.4.0 (2026-10-25)

- **Blocking tests.** Malware and phishing now show whether blocking was confirmed on the services' own test names;
  Check live feeds shows how much a resolver blocks. AdGuard and DNS4EU block pages are recognised.
- **Path tab.** A run keeps the path check as its own tab, with the traced route and handshake time to each resolver.
- **Probe board.** Resolvers are listed fastest cached lookup first, with a status tile per capability check, coloured
  by group, and pass counts per check.
- **Live CDN mapping.** The CDN tab shows each provider's edges as the run maps them.
- **Pick sheet.** The Overview picks are one row per goal: resolver, deciding figure, a short note and the graph the
  goal was decided on, with the full sentence on hover. Best from here closes the sheet as the total with the terms it
  adds up; a finished run's Overview no longer repeats the ranking, and a tall, wide window adds a query timeline.
- **Time.** A run of only your own servers shows its real two-second pace.

## 0.3.13 (2026-10-24)

- **Pick cards.** The Overview picks are compact cards: goal, resolver, deciding figure and a short note, with the
  full sentence on hover.
- **Cache and fresh-lookup picks.** Two more picks name the fastest resolver on cached answers and on fresh lookups; the
  speed score is the average of the two.
- **Failed answers cost.** Error replies and network failures count against the speed score like timeouts. A failure
  that hits three or more resolvers at once is excused as a network blip.
- **Speed score.** One slow answer no longer moves a resolver's score, and TLD lookups weigh less.
- **TLD test.** Queries are paced so the test no longer trips rate limits. A resolver that still refuses has that test
  marked limited and left out of its score.
- **Run status.** The check under way shows in the status bar, clear of the latency sheet.
- **Resolver details.** The panel uses the full width beside the shortlist, and CDN table values no longer wrap.
- **Sidebar.** Navigation, runs and monitoring read as three distinct sections.

## 0.3.12 (2026-10-24)

Security fixes from an audit, and dependency updates.

- **Browser UI token.** The CLI no longer puts its session token in the page it serves, where another user on the same
  computer could read it from the port, and no launch link carries a token. The link Recursa opens, and the one it
  prints for another browser, each carry a one-time code (`#code=`) that the page trades for its own session. A link
  opened twice ends the session it started, its live streams included, and Recursa prints a new one. The opened link
  expires after two minutes if no page used it. Exports send the token in a header, so it stays out of the download
  list.
- **TCP and DoT deadlines.** A server that sent a reply one byte at a time could hold a query, and so a whole run, for
  hours. Each exchange and TLS handshake now has a deadline.
- **NTS.** A forged or unverifiable reply, a Kiss-o'-Death or an unsynchronized one included, no longer ends an
  authenticated check; it is discarded, as RFC 8915 asks. Only an NTS NAK may come unauthenticated. A public NTS-KE
  server can't name a loopback, LAN or multicast address as its time server, and when it names no server, or itself,
  the queries go to the address the key exchange used. DNS query IDs, NTS identifiers and DNS cookies come from the
  system's secure random source.
- **Smaller fixes.** Feed downloads stop at 16 MB while they are read. Incident links from status pages must be https.
  The data directory is private (0700) on macOS and Linux and is never in the shared temp directory. The desktop app
  opens other `localhost` pages in the browser, not in its window, and its Settings window's outside links open in the
  browser again. Recursa doesn't query local, multicast or broadcast addresses a resolver under test gives it (Deep
  Dive, CDN mapping, cached-test TTLs). A backward clock step can't wrap a round trip. `npm run dev` works again.
- **Dependencies.** rustls 0.23.46, rustls-webpki 0.103.16 and webpki-roots 1.0.11 fix eight RustSec advisories. Tauri
  2.12.5, tokio 1.54.1, Vite 8, TypeScript 6 and Vitest 5. CI builds on Node 26 with its actions pinned by commit.
  quick-xml 0.39 (RUSTSEC-2026-0194, -0195), used by Routinator's RRDP client, waits for a new `rpki` release.
- **Pending fixes.** `docs/PENDING.md` lists what waits on upstream. CI runs `cargo audit`, which fails on any new
  advisory, and `tools/pending.py`, which reports each pending item in the run summary and warns once its fix is out.

## 0.3.10 (2026-10-22)

Refinements to the resolver detail introduced in 0.3.9.

- Selecting a resolver now shows a compact shortlist (name, score bar) beside the detail pane instead of a floating
  overlay with a resize grip. ↑↓ still walk the list; Esc or the list header restores the full view. Phones keep the
  sheet.
- **Where the time goes** uses a linear axis in milliseconds beyond the wire; the wire's own value moves to the legend.
- **Where it answers from** keeps the round trip, time strip and path together, with egress, service network, RPKI and
  CDN mapping under an "Addresses and routing" heading.

## 0.3.9 (2026-10-22)

- **Resolver page and Inspector redesign.** The Inspector and the new `/resolver/:id` page share one layout: a verdict
  line, then sections for where it answers from, latency, behaviour, blocking, reliability, usage limits and policy.
  Sections with a problem come first and open; the rest fold to a one-line summary.
- **Checks where their evidence is.** Trace path, Sample 18 queries, Check behaviour and Check live feeds are buttons on
  their sections, with a status line while they run and their result kept for the session. **Run all checks** runs the
  first three in turn. One check runs at a time; the others say why they wait. The Deep Dive page is gone;
  `/deep-dive/:id` opens the resolver page.
- **Keyboard.** ↑↓ move the selection through the resolvers on screen, ⌘↵ opens the selected resolver's page.

## 0.3.8 (2026-10-21)

- **Trace route.** A button in the Inspector traces the path to the resolver over ICMP and draws every hop, its
  network, and each round's round trip. Also `recursa trace <resolver|ip>`. Windows needs Administrator. By Trippy
  (trippy.rs).
- **Where the time goes.** The ladder marks each resolver's wire floor (best TCP handshake), and the Inspector splits
  cached and uncached medians into wire, resolver and recursion. The floor is now the best of five handshakes.

## 0.3.7 (2026-10-18)

- The About screens show the same tagline on every platform: "Find the fastest, best-placed DNS resolver for your
  network." Windows showed older text, macOS none, and the iPhone app now shows it in Settings › About.

## 0.3.6 (2026-10-17)

- **Blocking checks in a run use published test domains**: `malware.testcategory.com` and `isitblocked.org` for
  malware, `phishing.testcategory.com` and `internetbadguys.com` for phishing, `nudity.testcategory.com` and
  `exampleadultsite.com` for adult content. They are what Cloudflare, Quad9 and Cisco OpenDNS publish for testing their
  own blocking, and replace live malware and phishing hosts and well-known adult sites. Malware, phishing and ad
  checks are on by default; the adult-content check stays off until you turn it on, since the network can see its
  lookups.
- **Deep Dive: blocking against live threat feeds.** An opt-in check asks one resolver for up to 20 live URLhaus
  malware and 20 OpenPhish phishing hosts that still resolve elsewhere, and shows the share it blocked, each host as
  blocked or resolved, and the malware hosts grouped by how long URLhaus has listed them. Resolvers checked within 30
  minutes of each other get the same hosts and line up in a table, so their blocking compares directly. A warning beside it
  says that security tools on the network may flag the lookups or block the device. The names go over DoT or DoH, or
  over plain DNS to your own resolver on the local network or when you allow it.
- **Deep Dive: deeper checks.** About 90 queries for test names, started by hand, check what a run doesn't: whether
  a large signed answer is truncated cleanly when it's larger than a 512- or 1232-byte buffer and comes whole over
  TCP; whether the resolver caps long and short TTLs, compared with what the zone's own servers publish; how long it
  keeps a nonexistent name; how it handles DNSSEC across algorithms 8, 13, 15 and 16 and with expired signatures or a
  mismatched DS, with the EDE codes it returns; how many separate caches answer behind the address; and what TCP, DoT
  and DoH connection setup costs against UDP.
- Results show **Not tested** for a blocking check the run left off, and the Capabilities verdict no longer says no
  resolver blocks malware when malware blocking wasn't tested.

## 0.3.5 (2026-10-17)

- **Kernel receive stamps on Windows**: UDP answers are stamped by the network stack as they arrive
  (`SIO_TIMESTAMPING`, Windows 10 2004 and later) instead of when the measurement thread wakes up, and the engine's
  clock is the performance counter those stamps use. Windows stamps a packet where the adapter hands it up, so
  this needs the adapter's software timestamping turned on; loopback answers, older Windows and adapters without
  it keep the userspace stamps. Recursa asks the adapter whether it stamps, and the calibration and the Method
  page say which applied and how to turn stamps on.

## 0.3.4 (2026-10-17)

- **EDNS Client Subnet** is checked against four authorities (Akamai, Fastly, Google, PowerDNS) instead of two, so a
  resolver that sends your subnet only to some CDNs shows **Selective** rather than passing as "Sends /24" or
  "Not sent". The check's evidence, the Inspector's CDN mapping line and the CDN matrix chip list what each
  authority saw.
- **Run actions where the runs are**: each run in the sidebar shows a bookmark (save) and a "…" menu on hover
  (Open, Export, Save, Delete); the Runs heading has a "…" menu with **Clear All Runs…** and a link to the
  history settings. The Results toolbar carries the same bookmark and menu, replacing the export pop-down, the
  trash button and the Inspector's Save button. Delete or Backspace on a focused run deletes it.
- **Settings window**: a resizable window with sections down the left (General, Benchmark, Monitoring, Run
  history, Route origins, About) instead of one long scroll, always opening at its designed size. About shows the
  mark. The desktop File menu gains **Save Run** (⌘S / Ctrl+S), **Delete Run…** (⌘⌫ on a Mac) and
  **Clear All Runs…**.
- **Measurement conditions** opens above the chevron that opens it, at the right end of the status bar.
- **Check progress**: the progress rail sweeps only until a check reports, then fills with what it reports; the
  line naming the check no longer animates on its own.

## 0.3.0 (2026-10-17)

- **Check progress**: after the latency rounds, a progress line names the check under way (path check, capability
  probes, CDN mapping, network intel) with a sweeping segment on the progress rail. The segment holds still when
  reduced motion is on.
- **Run housekeeping**: **Save Run** in a run's sidebar menu, or the Save button in the Inspector, keeps a run
  for good; **Clear All Runs…** deletes every unsaved run after confirming how many go. Settings gains a **Run
  history** section that deletes old unsaved runs automatically, keeping the last 50 and the last 90 days by
  default, each rule switchable off. Cleanup runs when Recursa starts and after each run, never during one. A
  database with runs from an earlier version starts with both rules off.

## 0.2.1 (2026-10-17)

- **Accurate mode** keeps one query in flight per resolver with sends 4 ms apart, so a run with many resolvers
  finishes in a fraction of the time and a slow resolver no longer delays the others.
- **Per lookup** column and **Best from here** pick: one number per resolver that combines speed, a traffic-mix
  setting (Home, Office, Lab), the measured cache-hit rate and the CDN edge penalty. The speed score and the other
  picks are unchanged.
- **CDN mapping** asks each CDN for several hostnames, scores penalties first against a measured noise floor, and
  shows tiers and coverage. Runs recorded by earlier builds show the CDN tab's no-data state.
- **Time tab** setup is one sheet (presets, query count, NTS toggle, grouped server list) and the run is drawn as it
  goes, one lane per server.
- **Tooltips** sit under short elements, drop on a scroll and come back for whatever is under the pointer once it
  settles.
- **Windows**: the engine finds the system resolvers, gateway and link through the IP Helper API instead of parsing
  `ipconfig`, measurement threads run at time-critical priority, a start-up failure shows a dialog, and the Windows
  build runs the full test and lint suite. Receive stamps are taken in userspace there (the Method page says so).
  The link check pings the gateway through `IcmpSendEcho2`, timed in userspace; it reports stalls and loss but
  not the loopback echo or interface counters the macOS check reads.
- Cleanup: unused result fields, two API routes (`GET /api/resolvers`, `POST /api/env/refresh`) and `ntp --json` are
  gone (`ntp --format` stays); `--port` and `--no-open` go before the subcommand; the desktop app takes `--db=path`;
  Copy works on Windows. The Windows tray shows the app icon, a left click on it opens the window, the menus hold
  only the items Windows has, and F11 toggles full screen.

## 0.1.0-rc1

The first public version.

- **Recursa.app** for macOS and the **`recursa`** command line, built from one engine and one interface. The command
  line runs benchmarks in the terminal or serves the same interface to your browser on 127.0.0.1.
- **Latency** of cached, uncached and TLD lookups over UDP, TCP, DoT and DoH, with kernel receive timestamps on macOS
  and Linux and a published self-calibration floor.
- **The latency ladder** draws each lane's distribution as a density ridge under its samples, with a crosshair that
  reads any point on the axis. During a run every target holds its row from the start: warm-up answers show as rings,
  new samples stream in and land at their value, rows slide into rank as their first answers arrive, and the axis holds
  steady when measuring begins.
- **Capabilities**: DNSSEC validation, EDNS(0), ECS, QNAME minimisation, NSID, DNS Cookies, Extended DNS Errors,
  NXDOMAIN rewriting, rebinding protection, and TCP/DoT/DoH/IPv6 reachability, each with its evidence. The
  Capabilities view leads with one sentence on which resolver protects best and which is most private, names the gaps,
  and counts who answers over encrypted DNS and who passes your subnet on. Below it, one sheet with a row per capability
  and a column per resolver: a row where every resolver agrees collapses to one line, blocking shows how many test
  hosts each one blocked, and a run without probes says so instead of showing an empty grid.
- **Blocking** checks with live malware and phishing hosts, sent only over encrypted DNS, or over plain DNS to your own
  resolver on the local network (a Pi-hole, your router) so you can check that its blocking works. Your own resolver
  keeps Best protection and Most private when it qualifies, and Most private puts resolvers that block trackers first.
- **CDN mapping**: each resolver's CDN answers are timed and compared with the answer each CDN gives this computer
  directly, and the table shows each network's mark above its name. Nearest CDN edges is a pick of its own, beside fastest answers.
- **Interception** detection for port 53, **RPKI** and route-origin validation, live operator status and an incident
  history. Status is judged for where you are: a report about another region (Quad9's "Service degradation in Asia",
  say) leaves a resolver operational in your country, with the report in one line beside it; a report that names no
  place counts for everyone.
- **Deep Dive**: a short, read-only look at one resolver, from the Inspector or its menu: the spread of its latency,
  cold versus warm cache, how long answers stay cached, and each response's fields (rcode, flags, EDNS, DNSSEC, EDE, NSID).
- **Link check**: while a run measures, Recursa pings your gateway and records how its replies behaved. A run on a stalling
  link (Wi-Fi scans, a busy channel) says so, and the status bar shows the link's typical time and 95th percentile.
  On macOS the warning also names the likely cause: a busy Mac, other traffic, AirDrop, a driver fault or the Wi-Fi link.
- **Background monitoring** with an hourly history and an outage record. Each cycle tests both servers of a resolver
  (primary and secondary) and weighs them together into one line. The Reliability page leads with one sentence on what
  the record shows (who stopped answering, how often, and the hours Recursa wasn't running), then draws each resolver's
  answer times as a distribution over time, not an average: a cache's fast answers and the slower lookups show as two
  bands, with the median, the share left unanswered, outages and operator-reported incidents marked on the same axis.
  Outages are listed with an honest bound on how long each lasted.
- There is no overall ranking: each goal (fastest answers, nearest CDN edges, best protection, most private, steadiest) has its own pick. The
  speed score weighs consistency and cached answers more: cached 40%, uncached 35%, TLD 25%, and each test counts its
  median pulled halfway toward its 95th percentile with a quarter of its standard deviation added. A CDN edge farther
  away is described in round trips per new connection, three at least, rather than folded into the speed score. A dnsmasq forwarder on your network (a Pi-hole,
  or your router's DNS proxy, found by what it says it is or as the default gateway) is shown as a baseline and never
  named the best pick; your ISP's resolvers and any other resolver still compete. The Method page explains dnsmasq. A resolver with no reliability record gets a note
  under the verdict, and the transport badges (UDP, TCP, DoT, DoH) on the Latency view are filled in distinct colors.
- The names the Cached test asks are listed in the Test plan, where you can add and remove them, load a file of your own
  (one name per line; hosts files and URLs work) and keep named lists. `recursa run` takes `--cached-names` and
  `--cached-names-file`. A name that doesn't resolve is left out of the run, and the run says so.
- Each answer's TTL is recorded and used to check the tests. An Uncached answer that comes back below the zone's full TTL
  came from a cache: it is left out and the run says so. The Cached test shows how often each resolver really answered from
  cache, judged against the TTL the zone's own name servers give, and warns when custom names often miss.
- The Uncached test spreads its names over four zones on different DNS hosts (NS1, Vercel, Google Cloud DNS and Route 53).
  At the start of each run it checks that every zone is still unsigned and answers any name, and drops one that isn't.
- The TLD test asks under `.com`, `.org` and `.uk` in turn (three registry operators). At the start of each run it checks that
  each TLD still proves names don't exist with opt-out NSEC3, which no resolver may reuse for another name, and drops one
  that doesn't.
- An answer of 0.0.0.0, 127.0.0.1, `::` or `::1`, or a blocking Extended DNS Error, counts as blocked, not as a fast answer.
  So does a TLD-test NXDOMAIN without the TLD's own SOA record. When a resolver blocks some of the Cached test's names
  (a Pi-hole with your own list, say), the run names them and says whether that kept the resolver out of the ranking.
- Timeouts count in the ranking: each adds 1 s spread over every query sent, so a resolver that drops 1% of queries carries
  10 ms more. When two resolvers are too close for the run's own variation to separate, the verdict calls it a tie, and the
  sidebar names both ("A or B"). The
  Method page explains both, and how the Cached and Uncached tests behave on resolvers that run many caches.
- **Time**: an NTP test page. It asks about 60 public time servers (Google, Cloudflare, Meta, Apple, Microsoft, Amazon, NIST, USNO, PTB,
  Netnod, NPL, NICT and other national labs, and the pool's regional zones) for the time, eight seconds apart, and reports each one's offset, its error bound,
  latency, jitter, stratum and source, and how far it sits from the consensus of all of them, which is also how far your own clock is off. It is gentle with servers:
  at most 8 packets to an address, 8 seconds apart and staggered, no second look at a name within 10 minutes, no more queries to an address that answers
  none of three, and a Kiss-o'-Death is obeyed and remembered (an hour for RATE, a week for DENY or RSTR). A Features view sets
  leap-second handling, anycast, policy, IPv6 and a live NTS key-exchange check side by side. `recursa ntp` does the same in a terminal.
  NTS is checked with a full authenticated exchange. You can add servers of your own (they are measured and judged but never vote), export CSV or
  JSON with a `schema_version`, and use `recursa ntp --plugin` as a Nagios-style check (exit 0 to 3). Latency is timed to the kernel's receive stamp,
  and servers that fail together (one operator, or one upstream) count as one voice in the consensus. An Over time view can watch up to eight servers in
  the background (every 15 minutes or more) and chart each one's uptime, offset and delay, and your clock's drift.
- Rate limits: Google's and Cloudflare's published limits are shown beside their resolvers (only what each publishes, with its
  source). A run measures the most queries per second it sent to each resolver, warns when a published number is within reach,
  and flags a resolver that answered and then went silent or began refusing, setting the limit beside what was sent.
- When port 53 is answered locally, the check says what answered: a box that names itself dnsmasq is matched to your Pi-hole
  or gateway, replies that crossed no router point to your own link, TCP port 53 is compared with UDP, and a name the box has
  seen answering much faster than one it hasn't shows it keeps a cache.
- A run in progress shows the time elapsed and about how long is left, and says when timeouts make it run long. Screen
  readers hear the verdict once when a run finishes.
- A live run moves continuously between its two-second updates: the latency figures glide to each new reading, rows
  slide to their new rank, new answers land in place, and the axis eases to a new range. The warm-up shows its answers
  as hollow rings (they are never recorded) and fills its progress bar. With reduced motion on, everything updates in
  place.
- The Capabilities view can show one group at a time: Privacy & security, Blocking, Routing or Protocol.
- **Network intel**: the origin AS, BGP prefix and AS name come from Team Cymru's
  IP-to-ASN DNS service, asked through your system resolvers.
- **Route origins (RPKI)** are checked on this computer, and only if you turn it on in Settings. Recursa uses NLnet
  Labs' Routinator as a library, over RRDP only, to download and validate the five regional registries' route data, keeps a
  compact table of it, and applies RFC 6811 to each resolver's prefix and origin AS. The first sync takes 5 to 15
  minutes and downloads about 725 MB, and the data takes about 2.5 GB of disk; it refreshes every 12 hours, never during a measurement, and turning
  the setting off deletes it. Each sync runs in a short-lived helper process, so the memory it needs (about 1 GB) is
  given back when it ends. Off, the ROA cell says "Not checked". The ARIN trust anchor
  is covered by ARIN's Relying Party Agreement.
- Licensed under the GNU GPL, version 3 or later.
