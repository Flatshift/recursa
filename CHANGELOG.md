# Changelog

Notable changes to Recursa, newest first. Versions follow [Semantic Versioning](https://semver.org/).

## Unreleased

- **Kernel receive stamps on Windows**: UDP answers are stamped by the network stack as they arrive
  (`SIO_TIMESTAMPING`, Windows 10 2004 and later) instead of when the measurement thread wakes up, and the engine's
  clock is the performance counter those stamps use. Windows stamps a packet where the adapter hands it up, so
  this needs the adapter's software timestamping turned on; loopback answers, older Windows and adapters without
  it keep the userspace stamps, and the calibration and Method page say which applied.

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
