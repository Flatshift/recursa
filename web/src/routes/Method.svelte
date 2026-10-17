<script lang="ts">
  import { onMount, tick } from 'svelte';
  import Toolbar from '../components/Toolbar.svelte';
  import { app } from '../lib/store.svelte';
  import { api } from '../lib/api';
  import type { Env } from '../lib/types';
  import { us, when } from '../lib/format';
  import Icon from '../components/Icon.svelte';

  const env = $derived(app.env);
  const cal = $derived(env?.calibration);
  const m = $derived(app.boot?.method);
  let busy = $state(false);

  // A link from elsewhere can name a section (/method#politeness).
  onMount(async () => {
    await tick();
    const id = location.hash.slice(1);
    if (id) document.getElementById(id)?.scrollIntoView();
  });

  async function recalibrate() {
    busy = true;
    try {
      const e = await api.post<Env>('/api/calibrate');
      if (app.boot) app.boot = { ...app.boot, env: e };
      app.notify('Recalibrated.');
    } catch (err) {
      app.notify((err as Error).message, 'error');
    } finally {
      busy = false;
    }
  }

  const toc = [
    ['timing', 'Timing'],
    ['fairness', 'Fairness'],
    ['tests', 'The three tests'],
    ['forwarders', 'Your router or Pi-hole'],
    ['interception', 'Interception check'],
    ['probes', 'Capability probes'],
    ['cdn', 'CDN mapping'],
    ['intel', 'Network intel'],
    ['politeness', 'Rate limits and politeness'],
    ['ntp', 'Time (NTP)'],
    ['footprint', 'What Recursa sends where'],
    ['cli', 'Command line'],
  ];
</script>

<div class="view-root">
  <Toolbar title="Method" subtitle="How Recursa measures, and what it sends where" />
  <div class="view-body">
  <div class="content scroll wrap">
  <aside class="toc chrome" aria-label="On this page">
    <ol>
      {#each toc as [id, t]}<li><a href="#{id}">{t}</a></li>{/each}
    </ol>
  </aside>

  <article>
    <h1 class="page-title">How Recursa measures</h1>
    <p class="lede">
      This page tells how Recursa measures and what it sends where. It also shows how much time the tool itself adds to each
      result.
    </p>

    <section id="timing">
      <h2>Timing</h2>
      <p>
        Each plain-DNS (UDP) target has its own connected socket, on a measurement thread apart from the web server. Recursa encodes
        each query before the clock starts. It takes the send stamp immediately before the <code>send()</code> system call. The
        <b>kernel</b> gives the receive stamp when the packet arrives (<code>SO_TIMESTAMP_MONOTONIC</code> on macOS,
        <code>SO_TIMESTAMPNS</code> on Linux, <code>SIO_TIMESTAMPING</code> on Windows 10 2004 and later). The time that the
        scheduler needs to wake the thread is not part of the result. Windows stamps where the adapter hands the packet up, so
        it needs the adapter's software timestamping turned on. Where a stamp is unavailable, the receive stamp is taken when
        the thread reads the packet.
      </p>
      <div class="instrument">
        <dl>
          <div><dt>Clock</dt><dd class="mono">{env?.clock ?? '—'}</dd></div>
          <div><dt>Kernel receive stamps</dt><dd>
              {env?.kernel_rx
                ? env?.platform === 'windows'
                  ? 'On: answers are stamped where the adapter hands them up (loopback is not)'
                  : 'On'
                : 'Unavailable on this system: userspace stamps'}
            </dd></div>
          <div><dt>Loopback floor (median)</dt><dd class="num">{us(cal?.floor_p50_us)}</dd></div>
          <div><dt>Loopback floor (99th pct)</dt><dd class="num">{us(cal?.floor_p99_us)}</dd></div>
          <div><dt>Wake-up delay removed (median)</dt><dd class="num">{us(cal?.wake_p50_us)}</dd></div>
          <div><dt>Wake-up delay removed (99th pct)</dt><dd class="num">{us(cal?.wake_p99_us)}</dd></div>
        </dl>
        <div class="cal-foot">
          <p class="muted">Calibrated {cal ? when(cal.at) : 'never'} with {cal?.samples ?? 0} loopback queries.</p>
          <button class="btn sm" onclick={recalibrate} disabled={busy}><Icon name="refresh" size={13} />{busy ? 'Calibrating…' : 'Recalibrate'}</button>
        </div>
      </div>
      <p>
        The <b>loopback floor</b> sends queries through the same code path to a responder on this computer. The responder spins and
        does not sleep, so the floor is only the tool's send path plus two trips through the kernel's loopback stack. It is the
        maximum time that the tool can add. A difference smaller than the floor has no meaning.
      </p>
      <p>
        TCP, DoT and DoH keep a persistent connection. They use the same clock, with userspace stamps. Recursa times connection setup
        and TLS handshakes separately. They are never part of query latency.
      </p>
      <p>
        <b>A noisy link.</b> Each answer crosses your own link first. If the link stops for a moment (a Wi-Fi scan, a busy channel),
        all queries in that period are slow, to every resolver. The speed score uses the 95th percentile and the spread, so it
        increases too.
      </p>
      <p>
        To find this, Recursa pings the default gateway (usually your router) every 100 ms while a run measures. It times each reply
        with the same clock{env?.kernel_rx && env?.platform !== 'windows' ? ' and kernel stamps' : ', in userspace'}. A ping is a stall if it gets no reply, or if it is slower than both 20 ms and five times the
        link's median. If stalls are more than 2% of the pings, or the 95th percentile of the pings is more than 20 ms, the run shows
        a warning and the status bar highlights the link. Recursa does not change the scores. The warning tells you that they are too
        high: run again on a quieter link. If the gateway does not reply to ping, or there is no gateway, the run says that it did
        not check the link.
      </p>
      <p>
        On macOS, Recursa also gives the probable cause. It looks at what the Mac did during the stalls: a ping to itself and the
        timing of the check (a busy Mac), traffic on the network interface, AirDrop's awdl0 interface, and driver errors and drops.
        If none of these agrees, the cause is the Wi-Fi link.
      </p>
    </section>

    <section id="fairness">
      <h2>Fairness</h2>
      <ul>
        <li><b>Warm-up first.</b> Each target gets some queries that Recursa does not record. They fill the ARP, NAT and route caches, and put popular names into the resolver's cache. Recursa removes a target that never answers at this step, so the run does not wait on its timeouts.</li>
        <li><b>Interleaved rounds.</b> Each round sends one query to each target, in a new random order. Short congestion on your link thus hits resolvers at random, not the one under test at that moment.</li>
        <li><b>Identical work.</b> All resolvers get the same cached names. Uncached and TLD names are new for each query, but come from the same zones, so each one costs the same.</li>
        <li><b>Accurate pacing.</b> One query in flight per resolver, and sends to different resolvers at least 4 ms apart, so queries never wait in a queue on your own link and a slow resolver never holds up the others. Quick mode allows up to eight in flight with 1 ms between sends.</li>
        <li><b>Network silence.</b> During measured phases, Recursa sends no other network requests, except the gateway ping every 100 ms for the link check. Status pages, Team Cymru lookups, the location database download and threat feeds wait or use their cache.</li>
      </ul>
    </section>

    <section id="tests">
      <h2>The three tests</h2>
      <dl class="tests">
        <div>
          <dt><i class="key cached"></i>Cached</dt>
          <dd>
            Popular names ({m?.cached_names.join(', ')}). Most resolvers answer them from cache, so this test measures the network
            round trip plus the resolver's front end. Large services run many caches behind one address, and each query reaches one of
            them, so popular names also miss sometimes. The run compares the TTL of each answer with the TTL from the name's own server,
            and shows how often each resolver answered from cache. These are the default names. In the Test plan you can add and remove
            names, load your own file, or keep named lists. All resolvers get the same names. A name that does not resolve is removed
            from the run.
          </dd>
        </div>
        <div>
          <dt><i class="key uncached"></i>Uncached</dt>
          <dd>
            Random labels under unsigned wildcard zones ({m?.uncached_zones.join(', ')}). A different DNS provider hosts each zone.
            Each name exists, but no cache can have it. The zones are unsigned, so aggressive NSEC cannot give a shortcut. The resolver
            must ask the authoritative servers, and this is the delay that people feel. These servers are usually large anycast
            networks near the big resolvers, so for them the difference from Cached is often small. Setup makes sure that each zone is
            still unsigned and still answers all names, and does not use a zone that fails. A new answer has the full TTL of the zone.
            An answer with a lower TTL came from a cache: Recursa removes it and records this.
          </dd>
        </div>
        <div>
          <dt><i class="key tld"></i>TLD</dt>
          <dd>
            Random names that cannot exist, under <code>.com</code>, <code>.org</code> and <code>.uk</code> in turn (three different
            registry operators). The resolver already knows the servers for each TLD, so this test is one trip to them, not to the
            root. Each TLD proves that a name does not exist with hashed, opt-out NSEC3 records. A resolver cannot use these records
            to answer a different name. Setup makes sure that this is still true, and does not use a TLD that changed. An NXDOMAIN
            without the TLD's own SOA record was made on the path, and counts as blocked. An answer that is not NXDOMAIN shows
            NXDOMAIN rewriting.
          </dd>
        </div>
      </dl>
      <p>
        Statistics use only answered queries. Timeouts and errors decrease the “answered” rate. A block answer from a filter does too
        (0.0.0.0, a loopback address, or a “blocked” error code). If a resolver blocks some Cached names, the run names them. You can
        then remove them from the list to compare on speed only.
      </p>
      <p>
        There is no single overall winner. Each goal (fastest answers, nearest CDN edges, protection, privacy, steadiness) has its
        own pick, and a pick must have answered at least 98%. The latency ranking and the fastest pick use a speed score in ms:
        cached 40%, uncached 35%, TLD 25%. For each test, the score is the median moved halfway to the 95th percentile, plus a
        quarter of the standard deviation, so a steady resolver beats an erratic one. Each timeout adds 1 s, about the time your
        computer waits before it asks again, divided over all queries sent. If this run's own variation can cause a difference (the
        answers resampled 200 times), the result is a tie, and no resolver is first.
      </p>
      <p>
        Per lookup puts the speed figures, the measured cache-hit rate and the CDN edge penalty in one number: what one lookup costs
        on average from this network, in ms, at the traffic mix set in Settings (80% of lookups answered from the resolver's cache
        at home, 60% in an office, 40% in a lab). The cached figure is corrected for answers that were really fresh lookups, using
        the share of Cached answers the resolver gave from its cache. Uncached and TLD figures count for the rest, weighted as in
        the speed score. Timeouts add their cost, and so does the CDN edge penalty, 3 round trips on the half of lookups that open a
        new connection; without CDN mapping the number is DNS only. The Best from here pick ranks by it and may name a tie. At the
        Lab mix without the correction and the CDN term it is the speed score.
      </p>
    </section>

    <section id="forwarders">
      <h2>Your router or Pi-hole (dnsmasq)</h2>
      <p>
        Most home routers, many ISP modems and Pi-hole use <b>dnsmasq</b>, a small DNS forwarder, to serve DNS on your network.
        Your devices ask it: it is the 192.168.x.x or 10.x.x.x address that your DHCP gives. It answers from its cache when it can,
        and sends all other queries to the resolvers in its configuration (your ISP's, or a public one). It also gives DHCP leases
        and names your local devices. It does not resolve names itself, so the resolvers that you choose do the work behind it.
      </p>
      <p>
        <b>How Recursa finds it.</b> Recursa asks each local resolver for <code>version.bind</code> (a standard CHAOS query).
        dnsmasq replies with its name and version. Pi-hole, which uses dnsmasq, replies with <code>dnsmasq-pi-hole-…</code>. Some
        configurations do not reply. A resolver that is this computer's default gateway is always a router DNS proxy. Recursa
        treats nothing else as a forwarder. Your ISP's resolvers, a public resolver, or a real resolver on your network (Unbound,
        for example) are alternatives, and compete like all others, at any address.
      </p>
      <p>
        <b>Why it is a baseline, not a pick.</b> You cannot change to it: what you choose is the resolver behind it. Its cached
        answers come from your own network in a few milliseconds, and tell nothing about that resolver. With the high cached
        weight, it would always rank first. So it stays in the ranking as a reference line, marked <i>baseline</i>, and Recursa
        never names it the best resolver. It answers a different question: does <i>it</i> make your DNS slow?
      </p>
      <p>
        <b>Small devices.</b> Modems and cheap routers often run dnsmasq on a slow CPU with little memory. DNS shares that CPU with
        routing and NAT. Possible problems:
      </p>
      <ul>
        <li>Slow answers when the device is busy (a large download, many clients).</li>
        <li>A small cache. The dnsmasq default is 150 names, so repeated names miss and go upstream again.</li>
        <li>A limit on queries in flight (default 150), so bursts can be late or lost.</li>
        <li>Errors when it forwards large (EDNS) or TCP answers.</li>
        <li>No DNSSEC validation. Many router builds do not include it, and it is costly on a slow chip.</li>
      </ul>
      <p>
        Also, dnsmasq cannot use DoT or DoH upstream by itself, so the hop behind it is usually plain DNS. If the baseline's
        uncached or p95 numbers are much higher than those of a resolver measured directly, the device is the bottleneck. Set a
        better resolver in your DHCP settings, or put a more capable device in front, such as a Pi-hole on a Raspberry Pi.
      </p>
    </section>

    <section id="interception">
      <h2>Interception check</h2>
      <p>
        Some routers, firewalls and Wi-Fi access points answer outbound port-53 traffic themselves and do not tell you (DNAT
        rules, or AP features such as UniFi DNS Assistance). Recursa uses two independent signals. First, it times TCP handshakes
        to the address of each resolver. This gives the real network round trip, and no real answer can be faster. Second, it
        reads the IP TTL of each answer. A public resolver is many router hops away. An answer that crossed zero or one hop came
        from your local network, whatever its source address is.
      </p>
      <p>
        Recursa measures flagged targets but does not rank them. Probes and CDN lookups for them use DoT or DoH, which a device
        cannot answer transparently. To test port 53 from a terminal, run <code>recursa check</code>.
      </p>
      <p>When a device answers port 53 locally, Recursa also tries to identify it from four signs:</p>
      <ul>
        <li><b>Its name.</b> Recursa sends <code>version.bind</code> and <code>id.server</code> to the intercepted address. A real resolver gives a site code for <code>id.server</code> and nothing for <code>version.bind</code>. dnsmasq (on most home routers and Pi-hole) gives its version. Recursa compares this with the replies from your own Pi-hole and gateway.</li>
        <li><b>Its distance.</b> A reply that crossed no router came from a device on your own link (an access point, or the gateway). One router means a device behind the gateway.</li>
        <li><b>Its protocols.</b> A rule on a gateway usually catches TCP and UDP. If TCP port 53 gets to the real resolver but UDP is answered locally, the device is probably not the gateway.</li>
        <li><b>Its memory.</b> If a name that it saw before comes back much faster than a new name, it has a cache. The Cached test then measures that cache, not the resolver.</li>
      </ul>
      <p>These are signs, not proof. A device can answer CHAOS queries as it likes.</p>
    </section>

    <section id="probes">
      <h2>Capability probes</h2>
      <p>
        Probes run after the latency tests. They use random labels, so no cache can answer them, not even a transparent proxy.
      </p>
      <ul>
        <li><b>DNSSEC:</b> a name under <code>dnssec-failed.org</code> must fail. A name under the signed <code>ietf.org</code> should come back with the AD bit.</li>
        <li><b>EDNS:</b> the OPT record and payload size. A query with an unknown EDNS version must get the bad-version error, <code>BADVERS</code>.</li>
        <li><b>ECS and egress:</b> <code>o-o.myaddr.l.google.com</code> returns the address that Google's servers saw, and any client subnet that the resolver sent. <code>whoami.ds.akahelp.net</code> returns the same for Akamai. A resolver can send different data to each: Cloudflare, for example, sends Akamai a /29 and Google nothing.</li>
        <li><b>QNAME minimisation:</b> internet.nl's <code>qnamemintest</code> zone.</li>
        <li><b>Identity:</b> NSID and the <code>id.server</code> CHAOS TXT record name the instance that answered.</li>
        <li><b>Blocking:</b> 10 live malware hosts (URLhaus) and 10 live phishing hosts (OpenPhish), plus vendor test names. Recursa first makes sure that each live host resolves through an unfiltered, encrypted baseline. A block is NXDOMAIN, REFUSED, an empty answer, a sinkhole address, a known block page or an EDE code 15–18. Recursa sends live threat names only over DoT or DoH. Security appliances can see plain-DNS lookups of malicious names, and can block the computer that sends them. Your system resolver on the local network (a Pi-hole, your router) also gets the test over plain DNS, so you can make sure that its blocking works. You can turn this off. Other resolvers that use only plain DNS get the test only if you allow it.</li>
        <li><b>Integrity:</b> NXDOMAIN rewriting, private-address rebinding, Extended DNS Errors and DNS Cookies.</li>
        <li><b>Transports:</b> TCP, DoT, DoH and IPv6 reachability.</li>
      </ul>
    </section>

    <section id="cdn">
      <h2>CDN mapping</h2>
      <p>
        CDNs that steer by DNS choose an edge from the address that the query comes from. This is your subnet if the resolver sends
        EDNS Client Subnet and the CDN accepts it. If not, it is the resolver's own location. Each resolver looks up several
        hostnames for each CDN, two times, interleaved, because CDNs rotate their answers. Recursa then times TCP handshakes to
        each edge returned: three passes, shuffled, keeping the minimum.
      </p>
      <p>
        The penalty is set for each hostname first: the extra round trip compared with the best edge that any resolver (or the
        CDN itself) found for that same name, so a name that is far from everyone costs no one. A CDN's penalty is the median over its
        hostnames, and a resolver's is the mean over the steered CDNs that every ranked resolver has. Anycast controls are shown,
        not counted.
      </p>
      <ul>
        {#each m?.cdns ?? [] as c}
          <li><b>{c.name}:</b> {c.hosts.join(', ')}</li>
        {/each}
      </ul>
      <p>
        Recursa also asks each CDN's own nameserver directly from this computer, with no resolver between them. It uses only
        answers marked authoritative, so a device that intercepts port 53 cannot give a false one. That answer is the edge the CDN
        chooses for <b>your own address</b>. A resolver that sends a usable ECS should get the same edge. The table marks each
        resolver whose edges match it (● within about 4% or the run's noise) or nearly match it (○ in the same /24 cluster and
        within 10%, for Akamai, CloudFront and Meta).
      </p>
      <p>
        This reference shows why ECS does not always win. With ECS, a CDN can map you by your subnet, but only if it accepts ECS
        from that resolver (Akamai ignores some, CloudFront accepts more). Also, the CDN's map for your subnet can choose a slower
        edge than a well-connected hub near a resolver. If the reference itself is slow, all ECS resolvers get the same slow edge.
      </p>
      <p>
        The ECS column shows what each resolver tells CDNs: your subnet (<b>/24</b>), your subnet to Akamai only, a subnet that is
        not yours (<b>Other</b>), or <b>None</b>. <b>On-net</b> means that its queries leave from your own ISP. The table also marks
        edges inside your ISP's network (embedded caches).
      </p>
    </section>

    <section id="intel">
      <h2>Network intel</h2>
      <ul>
        <li>
          <b>RPKI:</b> checked on this computer, only if you turn it on in Settings. Recursa downloads the signed route data of the five regional
          registries over RRDP and validates it, with built-in code that works like
          <a href="https://www.nlnetlabs.nl/projects/rpki/routinator/" target="_blank" rel="noreferrer">NLnet Labs' Routinator</a>. It keeps a compact
          table of route origins, and applies RFC 6811 to the resolver's prefix and origin AS. It does not read publication points that offer only
          rsync. When RPKI is off, the cell shows "Not checked".
        </li>
        <li><b>Route-origin validation:</b> RoVista's measurement of how much of the operator's network drops RPKI-invalid routes.</li>
        <li>
          <b>Networks and holders:</b> IP-to-ASN mapping by <a href="https://team-cymru.com/community-services/ip-asn-mapping/" target="_blank" rel="noreferrer">Team Cymru</a>,
          over DNS: the origin AS and prefix of an address, and the AS name.
        </li>
        <li><b>Status:</b> the Statuspage feeds of Cloudflare, AdGuard and Cisco Umbrella, and the Better Stack feed of Quad9. The page shows which operators have no feed.</li>
      </ul>
    </section>

    <section id="politeness">
      <h2>Rate limits and politeness</h2>
      <p>
        Free public resolvers share their capacity. Some say that they will limit a client address that sends too much. Recursa
        sends each resolver one query at a time (sends go out at least 4 ms apart in Accurate mode, 1 ms in Quick), so a run sends a resolver tens
        of queries per second at most, in tests of 3 to 500 queries per resolver (30 by default). Monitoring sends six queries per
        resolver in each cycle, three to each of its two servers.
      </p>
      <p>
        <b>What operators publish.</b> Google applies rate limits per client address (an IPv4 address or an IPv6 /64). Its
        troubleshooting guide gives 1,000 queries per second as a level that can start throttling. The free service has no SLA.
        Cloudflare gives no number. It says that it can limit security scanning, proxied traffic and a high rate of uncacheable
        answers (SERVFAIL for one domain, for example). Quad9, OpenDNS, CleanBrowsing, Control D and AdGuard's public servers
        publish nothing that we could find, so Recursa shows no limit for them and does not guess. The free NextDNS plan counts
        queries per profile, and the addresses here have no profile. The Usage limits of each resolver link to the sources.
      </p>
      <p>
        <b>What Recursa detects.</b> After the measured tests, Recursa finds the highest number of queries sent to each resolver in
        one second, all transports together, because limits apply per address. It warns you if a published limit is near. It also
        looks for a resolver that answered normally and then stopped: no answers, refused queries, or SERVFAIL for the rest of a
        test. Throttling looks like this from here, but a network failure looks the same. The note shows what Recursa sent next to
        the published limit, so you can decide which is more probable. All devices behind one router share the allowance of that
        address, so a busy network leaves less capacity than a run alone.
      </p>
    </section>

    <section id="ntp">
      <h2>Time (NTP)</h2>
      <p>
        The Time page asks public NTP servers for the time and compares them. One exchange records four times: T1 when this computer
        sent the request, T2 when the server received it, T3 when the server replied, and T4 when the reply arrived. The
        <b>offset</b> is ((T2 − T1) + (T3 − T4)) / 2: how far the server's clock is ahead of yours. The <b>latency</b> is
        (T4 − T1) − (T3 − T2): the round trip minus the server's own time.
      </p>
      <p>
        T1 and T4 come from this computer's clock. T4 is T1 plus a monotonic round trip, so a clock adjustment cannot change it. The
        round trip ends when the kernel receives the reply (a socket receive timestamp), not when the program reads it. So a busy
        computer does not add its own scheduling delay to the server's latency. If the system gives no such stamp, Recursa uses the
        time when the program reads the reply.
      </p>
      <p>
        <b>What accuracy means here.</b> No network measurement can give the true time. The offset is exact only if the request and
        the reply took the same time, and no client can know this. So the error is at most half the round trip: the <b>±</b> column.
        Each server's offset comes from the query with the smallest delay, as NTP's own clock filter chooses it. <b>Error ≤</b> adds
        what the server reports about itself: half its root delay plus its root dispersion, the error from the clocks above it. That
        sum plus the ± is NTP's root distance (RFC 5905), the maximum distance of the offset from UTC.
      </p>
      <p>
        The <b>consensus</b> is the median offset of the servers that can vote: stratum 1 to 3, at least half of the queries
        answered, and pinned to within 50 ms. Before the median, NTP's own selection (RFC 5905) makes an interval from each voter's
        offset ± its error ceiling, and keeps the intervals that overlap with a majority. A <b>falseticker</b> cannot be correct if
        the majority is correct. Recursa measures it against the consensus, but it does not vote. Voters that would fail together
        count as one. Servers of one operator, and servers with the same upstream (the same reference ID at stratum 2 or 3), share
        one vote in the median. A verdict needs three of these independent voices, not three servers. With fewer than three, there
        is no verdict on your clock.
      </p>
      <p>
        Your clock's error is that median. Its uncertainty is not how closely the voters agree, because Recursa reaches all of them
        over your connection and they share its error. So the ± in the headline is the smallest error ceiling among them, and a
        median inside it shows as “within the noise”. Each server's “vs others” is its distance from the median of all the other
        servers, so a server at the median cannot agree with itself. Your own clock cancels out of this value, so it tells nothing
        about your computer. A server agrees if that distance is within its own error ceiling plus the consensus's.
      </p>
      <p>
        “Tightest error bound” names only servers that agree and are pinned to within 25 ms. From these, it names the one with the
        smallest error ceiling, not the one nearest the median: a position near the median only shows a shared path. Recursa flags a
        server whose addresses differ by more than their ceilings allow, because one of them is wrong. <b>Jitter</b> is the RMS
        distance of the other offsets from the best one (as in RFC 5905), so it needs two answers. The 95th-percentile latency shows
        only from eight answers. A server's headline numbers are the median over its addresses (up to four). A pool is servers from
        many volunteers, so its row shows how they vary.
      </p>
      <p>
        <b>Recommendation.</b> “Use together” chooses up to four truechimers that agree with the consensus and are synchronized to a
        clock within three steps. It sorts them by error ceiling plus jitter (NTP's synchronization distance, so a nearer server
        often wins). It takes a maximum of one server per operator and one per upstream. The reference ID of a stratum 2 or 3 server
        is the address of its upstream, so two servers with the same ID fail together. It does not name pools, or servers that smear
        the leap second (see Features).
      </p>
      <p>
        “Leave out” lists falsetickers, servers that told us to stop, servers not synchronized to a near clock, servers whose
        addresses disagree, and servers that report more than 50 ms of their own error. A server that did not answer is not blamed,
        because the cause can be this network. The <b>Fit</b> column shows the status of each server: in the set (numbered, best
        first), a spare (a better server has the same operator or upstream), too far to rank, or left out. The “Your clock” line
        tells what your clock's error means for logins, logs and Kerberos.
      </p>
      <p>
        <b>Features.</b> The Features view shows what each operator publishes (leap-second handling, anycast, policy) next to what
        Recursa measured: stratum and source, IPv6 answers from here, and whether the NTS key exchange (RFC 8915) completes. For
        NTS, Recursa opens the TLS 1.3 connection on port 4460 with a verified certificate and derives the session keys. Then it
        sends one authenticated time request, with a cookie that the server just gave. “Verified” means that the reply had a valid
        authenticator over our own unique ID: it came from the named server, and nobody forged or replayed it. Recursa still
        measures the offset over plain NTP, so this does not tell how accurate NTS time is. Each failure has a name: key exchange
        refused, certificate not valid, no cookies, or a reply that was not authenticated.
      </p>
      <p>
        A server that smears the leap second (Google over 24 hours, Meta over 17) is different from UTC for hours near a leap
        second. Do not mix it with servers that step. A blank means that the operator publishes nothing that we found. It does not
        mean no.
      </p>
      <p>
        <b>Your own server.</b> A host that you type on the Time page (<code>host</code> or <code>host:port</code>, up to four)
        gets the same measurement as all others. But it never votes, and the recommendation never names it, so it cannot move the
        consensus that it is compared with. If a run has only your hosts, Recursa adds four default references for comparison, and
        the page tells you. Because you chose the host, Recursa asks it at 2-second intervals, not 8, and you can run it again after
        30 seconds. Recursa still obeys a Kiss-o'-Death from it.
      </p>
      <p>
        <b>Failures</b> have names, not only “no answer”: no DNS, no address, timed out, kiss-o'-death, unsynchronized, bad reply,
        network error, and for NTS the step that failed. <b>Export</b> writes CSV or JSON from the page, or
        <code>recursa ntp --format csv|json</code>. Both include a <code>schema_version</code> and use the same failure names.
        <code>recursa ntp --plugin --servers HOST -w MS -c MS</code> prints one Nagios-style line and exits 0, 1, 2 or 3 (OK,
        warning, critical, unknown). The thresholds apply to the part of the offset that the round trip cannot explain, so a
        distant server is not flagged only for its path. This is a single check. The Over time view keeps a record.
      </p>
      <p>
        <b>Over time.</b> A run is one moment. When you turn on the Over time view on the Time page, Recursa asks a few servers that
        you choose (up to eight, up to four of them your own). It sends one query per address every 15 minutes or more (30 by
        default) while Recursa is open, and keeps the answers for the number of days you set. For each server it shows:
        checks answered (%), dropouts and returns, how steady its offset is from here, median and 95th-percentile delay, and delay
        by hour of day (UTC). The hourly delay follows how busy your own path is.
      </p>
      <p>
        It also draws the consensus of each check as a line, with its spread. After six hours and eight checks, it shows the slope
        in milliseconds per hour. A flat line near zero is a clock that keeps time. A slope is a clock that drifts between
        corrections. A check with fewer than three independent voices gives no point. The view sends fewer queries than a run, never
        more than one in 15 minutes (the run limit is 10). It uses the same record as runs, so it does not ask a server that told us
        to stop. It pauses while a run or a different Recursa measures.
      </p>
      <p>
        <b>Gentleness.</b> A time server is someone's infrastructure, often a volunteer's. So a run looks like an NTP client that
        just started, and is not heavier. It sends a maximum of 8 packets to an address (4 by default), never closer than 8 seconds
        apart. (ntpd rate-limits a client that averages more than one query in 8 seconds, and NIST asks for no more than one in 4.)
        Each address starts at a random time within those 8 seconds, so the run is not one burst. If an address does not answer
        three queries in sequence, Recursa does not ask it again.
      </p>
      <p>
        After Recursa asks a name, it does not ask it again for 10 minutes. This keeps Recursa at a maximum of 48 packets per hour
        to an address, fewer than the 56 that a normal client sends when it polls every 64 seconds. Recursa obeys a Kiss-o'-Death
        immediately: RATE (slow down) stops queries for one hour, and DENY or RSTR (go away) for one week. Recursa keeps this
        between runs, and between the app and the command line.
      </p>
      <p>
        Recursa measures a name with many addresses, such as the pool, at up to four of them. The list has the named public
        services, national time labs and the regional zones of the pool, not every volunteer in the pool. Each query also has a
        random value that the reply must echo, so a stray packet cannot look like an answer. Before you press Start, the Time page
        shows the maximum number of packets that a run can send.
      </p>
    </section>

    <section id="footprint">
      <h2>What Recursa sends where</h2>
      <p>
        The web interface is served only on <code>127.0.0.1</code>. It refuses all other Host headers, which blocks DNS rebinding.
        It also requires a token that Recursa makes at each launch. Outbound, Recursa connects only to the resolvers you select, the
        CDN edges that they return, the time servers you choose on the Time page (UDP 123, and TCP 4460 for the NTS check), and the
        public sources above. During a run it also pings your default gateway. A Deep Dive, which you start from a resolver's
        inspector, sends about 18 normal queries to that one resolver, one time. Results stay in a local database:
      </p>
      <p class="mono path">{env?.db ?? '—'}</p>
    </section>

    <section id="cli">
      <h2>Command line</h2>
      <pre class="mono"><code>recursa                     # open the interface
recursa run --samples 30    # benchmark the default set in the terminal
recursa run --resolvers quad9,cloudflare --transports udp,doh --json
recursa run --cached-names example.com,wikipedia.org    # your own names for the Cached test
recursa run --cached-names-file names.txt               # or a file of them
recursa check               # is port 53 direct or answered locally?
recursa calibrate           # measure the timing floor
recursa ntp --samples 4     # measure the default time servers (skips servers asked in the last 10 minutes)
recursa ntp --servers time.cloudflare.com,pool.ntp.org --no-nts   # chosen servers, without the NTS check
recursa ntp --all --format csv                          # every catalog server; table, json or csv
recursa ntp --plugin -w 50 -c 200                       # one-line monitoring check, exit 0 to 3
recursa list                # list resolver ids</code></pre>
    </section>
  </article>
</div>
  </div>
</div>

<style>
  .wrap {
    display: grid;
    grid-template-columns: 200px minmax(0, 1fr);
    gap: 48px;
    align-content: start;
  }
  .toc {
    position: sticky;
    top: 0;
    align-self: start;
  }
  .toc ol {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 2px;
    border-left: 1px solid var(--rule);
  }
  .toc a {
    display: block;
    padding: 4px 12px;
    color: var(--ink-2);
    text-decoration: none;
    font-size: var(--t-sm);
  }
  .toc a:hover {
    color: var(--ink);
  }
  article {
    max-width: 74ch;
  }
  .lede {
    margin-top: 10px;
    font-size: var(--t-md);
    color: var(--ink-2);
  }
  section {
    margin-top: 44px;
    scroll-margin-top: 80px;
  }
  h2 {
    font-size: var(--t-lg);
    font-weight: 650;
    margin-bottom: 10px;
  }
  p + p {
    margin-top: 12px;
  }
  ul {
    padding-left: 18px;
    display: grid;
    gap: 8px;
  }
  li {
    padding-left: 2px;
  }
  .instrument {
    margin: 18px 0;
    border: 1px solid var(--rule);
    border-radius: var(--r-lg);
    background: var(--surface);
    padding: 6px 18px 14px;
  }
  .instrument dl {
    margin: 0;
    display: grid;
    grid-template-columns: 1fr 1fr;
    column-gap: 28px;
  }
  .instrument dl div {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    padding: 9px 0;
    border-bottom: 1px solid var(--rule);
    font-size: var(--t-sm);
  }
  .instrument dt {
    color: var(--ink-2);
  }
  .instrument dd {
    margin: 0;
    font-weight: 600;
    text-align: right;
  }
  .cal-foot {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
    margin-top: 12px;
    font-size: var(--t-xs);
  }
  .tests {
    margin: 0;
    display: grid;
    gap: 14px;
  }
  .tests dt {
    font-weight: 640;
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .tests dd {
    margin: 2px 0 0 16px;
    color: var(--ink-2);
  }
  .path {
    font-size: var(--t-sm);
    padding: 8px 12px;
    background: var(--surface-2);
    border-radius: var(--r-md);
    word-break: break-all;
  }
  pre {
    margin: 0;
    padding: 14px 16px;
    background: var(--surface-2);
    border-radius: var(--r-md);
    font-size: var(--t-sm);
    overflow-x: auto;
    line-height: 1.7;
  }
  @media (max-width: 860px) {
    .wrap {
      grid-template-columns: 1fr;
    }
    .toc {
      display: none;
    }
    .instrument dl {
      grid-template-columns: 1fr;
    }
  }
</style>
