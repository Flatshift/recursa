import { describe, it, expect } from 'vitest';
import { cellPenalty, edgeIps, level, youOf } from './cdn';
import type { CdnInfo, Mapping } from './types';

// Trimmed from a version-2 report: one CDN with two hostnames, one resolver's mapping of it.
const you = (rtt_ms: number, penalty_ms: number) => ({ name: 'x.akamaiedge.net', server: 'ns (1.2.3.4)', ips: ['23.1.1.1'], rtt_ms, penalty_ms, error: null });
const host = (uniform = false) => ({ uniform, unexpected: false });
const akamai: CdnInfo = {
  id: 'akamai',
  name: 'Akamai',
  role: 'steered',
  best_ms: 14.4,
  hosts: [
    { name: 'www.apple.com', ...host(), you: you(40, 25) },
    { name: 'www.adobe.com', ...host(), you: you(30, 15) },
  ],
};
const hm = (ips: string[], same: boolean) => ({ ips, target: null, rotated: false, status: 'ok' as const, error: null, rtt_ms: 20, penalty_ms: 5, same, as_you: 'different' as const });
const mapping = (o: Partial<Mapping> = {}, same = [true, true]): Mapping => ({
  rtt_ms: 20,
  penalty_ms: 5.45,
  as_you: 'different',
  error: null,
  worst_host: ['www.apple.com', 13.2],
  hosts_used: 2,
  hosts_total: 2,
  hosts: { 'www.apple.com': hm(['23.62.1.1', '23.62.1.2'], same[0]), 'www.adobe.com': hm(['23.62.1.2', '23.62.1.3'], same[1]) },
  ...o,
});

describe('cdn helpers on a version-2 report', () => {
  it('lists each edge once across hostnames', () => {
    expect(edgeIps(mapping())).toEqual(['23.62.1.1', '23.62.1.2', '23.62.1.3']);
    expect(edgeIps(undefined)).toEqual([]);
  });

  it('shows the CDN penalty, or a control with one hostname its worst host', () => {
    expect(cellPenalty(akamai, mapping())).toBe(5.45);
    expect(cellPenalty(akamai, mapping({ penalty_ms: null }))).toBeNull();
    expect(cellPenalty({ ...akamai, role: 'control' }, mapping({ penalty_ms: null }))).toBe(13.2);
    expect(cellPenalty(akamai, undefined)).toBeNull();
  });

  it('is level only when every usable hostname is within the noise', () => {
    expect(level(mapping())).toBe(true);
    expect(level(mapping({}, [true, false]))).toBe(false);
    expect(level(undefined)).toBe(false);
  });

  it('reduces the CDN\'s own answers to their median', () => {
    expect(youOf(akamai)).toMatchObject({ name: '2 of 2 hostnames', rtt_ms: 35, penalty_ms: 20, error: null });
  });
});
