import type { Published, Resolver } from './types';

export type CapGroup = 'Security' | 'Privacy' | 'Blocking' | 'Protocol' | 'Transport' | 'Identity';

interface CapDef {
  key: string;
  label: string;
  group: CapGroup;
  /** One plain sentence: what it is and why it matters. */
  explain: string;
  ref?: string;
  /** Published claim from the catalog, when the operator documents one. */
  published?: (p: Published, r: Resolver) => boolean | null | undefined;
  /** For neutral capabilities, whether "present" is the desirable state. Undefined = neutral. */
  goodWhenPresent?: boolean;
}

export const CAPS: CapDef[] = [
  {
    key: 'dnssec',
    label: 'DNSSEC validation',
    group: 'Security',
    explain: 'Rejects answers whose signatures don’t verify, so forged records for signed zones never reach you.',
    ref: 'RFC 4033–4035',
    published: (p) => p.dnssec,
    goodWhenPresent: true,
  },
  {
    key: 'nxdomain',
    label: 'Honest NXDOMAIN',
    group: 'Security',
    explain: 'Names that don’t exist stay nonexistent instead of being rewritten to an ad or search page.',
    goodWhenPresent: true,
  },
  {
    key: 'rebind',
    label: 'Rebinding protection',
    group: 'Security',
    explain: 'Strips answers that point public names at private addresses, which blocks DNS-rebinding attacks on home devices.',
    goodWhenPresent: true,
  },
  {
    key: 'ecs',
    label: 'EDNS Client Subnet',
    group: 'Privacy',
    explain: 'Forwards part of your IP address to authoritative servers. CDNs map you more precisely, but more of your location leaves the resolver.',
    ref: 'RFC 7871',
    published: (p) => p.ecs,
  },
  {
    key: 'qmin',
    label: 'QNAME minimisation',
    group: 'Privacy',
    explain: 'Tells each server in the chain only the part of the name it needs, so the root and TLD servers don’t see full queries.',
    ref: 'RFC 9156',
    published: (p) => p.qmin,
    goodWhenPresent: true,
  },
  {
    key: 'filter_malware',
    label: 'Malware blocking',
    group: 'Blocking',
    explain: "Whether the resolver blocked a malware test name that a filtering service publishes (WICAR, Cloudflare, Quad9, Cisco or Google). Each service mostly blocks only its own, so this confirms blocking happens, not how much. The live threat test, a run option, measures that against live URLhaus and ThreatView hosts; Check live feeds does the same for one resolver on its page.",
    published: (p) => (p.filtering.includes('malware') ? true : p.filtering.length === 0 ? false : null),
  },
  {
    key: 'filter_phishing',
    label: 'Phishing blocking',
    group: 'Blocking',
    explain: "Whether the resolver blocked a phishing test name that Cloudflare or Cisco OpenDNS publishes. This confirms blocking happens, not how much. The live threat test, a run option, measures that against live OpenPhish hosts; Check live feeds does the same for one resolver on its page.",
    published: (p) => (p.filtering.includes('phishing') ? true : p.filtering.length === 0 ? false : null),
  },
  {
    key: 'filter_ads',
    label: 'Ad & tracker blocking',
    group: 'Blocking',
    explain: 'Whether common ad-serving and analytics hosts are blocked.',
    published: (p) => (p.filtering.includes('ads') ? true : p.filtering.length === 0 ? false : null),
  },
  {
    key: 'filter_adult',
    label: 'Adult content blocking',
    group: 'Blocking',
    explain:
      'Whether the adult-content test domains that Cloudflare and Cisco OpenDNS publish are blocked. Off by default; turn it on under Capability probes when setting up a run.',
    published: (p) => (p.filtering.includes('adult') ? true : p.filtering.length === 0 ? false : null),
  },
  {
    key: 'edns0',
    label: 'EDNS(0)',
    group: 'Protocol',
    explain: 'Extension mechanism that carries larger UDP answers, DNSSEC flags and options. Every modern resolver should speak it.',
    ref: 'RFC 6891',
    goodWhenPresent: true,
  },
  {
    key: 'edns_version',
    label: 'EDNS version handling',
    group: 'Protocol',
    explain: 'Answers a query with an unknown EDNS version with a "bad version" error (BADVERS), as the standard requires, so future extensions can be negotiated safely.',
    ref: 'RFC 6891 §6.1.3',
    goodWhenPresent: true,
  },
  {
    key: 'ede',
    label: 'Extended DNS Errors',
    group: 'Protocol',
    explain: 'Explains failures with a code (e.g. “DNSSEC Bogus”, “Blocked”) instead of a bare SERVFAIL.',
    ref: 'RFC 8914',
    goodWhenPresent: true,
  },
  {
    key: 'cookies',
    label: 'DNS Cookies',
    group: 'Protocol',
    explain: 'Lightweight transaction authentication that makes off-path spoofing and amplification harder.',
    ref: 'RFC 7873',
    goodWhenPresent: true,
  },
  {
    key: 'tcp',
    label: 'DNS over TCP',
    group: 'Transport',
    explain: 'Plain DNS on TCP port 53, required for large answers and as a fallback.',
    ref: 'RFC 7766',
    goodWhenPresent: true,
  },
  {
    key: 'dot',
    label: 'DNS over TLS',
    group: 'Transport',
    explain: 'Encrypted DNS on port 853. Nobody on the path can read or alter your queries.',
    ref: 'RFC 7858',
    published: (_, r) => !!r.dot,
    goodWhenPresent: true,
  },
  {
    key: 'doh',
    label: 'DNS over HTTPS',
    group: 'Transport',
    explain: 'Encrypted DNS inside HTTPS on port 443, which blends with web traffic.',
    ref: 'RFC 8484',
    published: (_, r) => !!r.doh,
    goodWhenPresent: true,
  },
  {
    key: 'ipv6',
    label: 'IPv6 reachability',
    group: 'Transport',
    explain: 'Answers on its published IPv6 address from this network.',
    goodWhenPresent: true,
  },
  {
    key: 'pop',
    label: 'Anycast site',
    group: 'Identity',
    explain: 'The server instance that answered, from CHAOS TXT id.server. Shows which location is serving you.',
  },
  {
    key: 'nsid',
    label: 'NSID',
    group: 'Identity',
    explain: 'Name Server Identifier option: another way the server tells you which instance answered.',
    ref: 'RFC 5001',
  },
];

export const CAP = Object.fromEntries(CAPS.map((c) => [c.key, c])) as Record<string, CapDef>;

/** The probed checks the Capabilities view and the live probe board both group, in column order. */
export const CAP_KEYS = {
  privacy: ['dnssec', 'ecs', 'qmin', 'rebind'],
  blocking: ['filter_malware', 'filter_phishing', 'filter_ads'],
  protocol: ['ede', 'edns_version', 'cookies', 'tcp', 'dot', 'doh'],
};

const liveExplain = (feed: string) =>
  `Real hosts from today's ${feed} that still resolve through an unfiltered resolver, looked up through each resolver by this run's live threat test. Every resolver got the same hosts.`;

/** The rows the run's live threat test adds to the Blocking group, shown only when the run has its results. */
export const LIVE_CAPS: { key: string; label: string; category: 'malware' | 'phishing'; explain: string }[] = [
  { key: 'live_malware', label: 'Live malware', category: 'malware', explain: liveExplain('URLhaus and ThreatView malware feeds') },
  { key: 'live_phishing', label: 'Live phishing', category: 'phishing', explain: liveExplain('OpenPhish phishing feed') },
];

export const PUBLISHED_ONLY: { key: string; label: string; explain: string; get: (p: Published) => boolean | null | undefined; ref?: string }[] = [
  { key: 'doq', label: 'DNS over QUIC', explain: 'Encrypted DNS over QUIC (UDP 853): TLS-grade privacy with lower handshake cost.', ref: 'RFC 9250', get: (p) => p.doq },
  { key: 'dnscrypt', label: 'DNSCrypt', explain: 'An older, non-IETF encrypted DNS protocol, still offered by some operators.', get: (p) => p.dnscrypt },
  { key: 'odoh', label: 'Oblivious DoH', explain: 'Splits who you are from what you ask via a proxy, so no single party sees both.', ref: 'RFC 9230', get: (p) => p.odoh },
];
