import { describe, expect, it } from 'vitest';
import { asrelMonth, clouds, linkLabel } from './netview';
import type { MapRun, NetGroup, NetLink } from './types';

const g = (id: number, role: NetGroup['role'], extra: Partial<NetGroup> = {}): NetGroup =>
  ({ id, role, label: '', asn: null, holder: null, ix_id: null, ix_name: null, ttl_from: 1, ttl_to: 1, rtt_ms: null, members: [], aliases: [], targets: [], ...extra }) as NetGroup;
const l = (to: number, kind: NetLink['kind'], ix_id?: number): NetLink =>
  ({ from: 1, to, count: 1, rtt_step_ms: null, kind, handoff: true, silent_hops: 0, ix_id }) as NetLink;

describe('linkLabel', () => {
  const groups = [g(1, 'isp'), g(2, 'exchange', { ix_id: 7, ix_name: 'Example IX' }), g(3, 'content')];
  it('does not repeat the name of the exchange node the link lands on', () => {
    expect(linkLabel(l(2, 'exchange', 7), groups)).toBe('at the exchange');
  });
  it('names the exchange when the link lands somewhere else', () => {
    expect(linkLabel(l(3, 'exchange', 7), groups)).toBe('Example IX');
    expect(linkLabel(l(3, 'exchange', 9), groups)).toBe('at an exchange');
  });
  it('names the other kinds plainly', () => {
    expect(linkLabel(l(3, 'direct'), groups)).toBe('direct');
    expect(linkLabel(l(3, 'via_transit'), groups)).toBe('via transit');
  });
  it('says what the relationship is when one is listed', () => {
    const r = (kind: NetLink['kind'], rel: NetLink['rel']) => ({ ...l(3, kind), rel });
    expect(linkLabel(r('via_transit', 'transit'), groups)).toBe('transit');
    expect(linkLabel(r('direct', 'peer'), groups)).toBe('peering');
    expect(linkLabel(r('direct', 'customer'), groups)).toBe('their customer');
  });
  it('keeps the exchange label ahead of a relationship', () => {
    expect(linkLabel({ ...l(2, 'exchange', 7), rel: 'peer' }, groups)).toBe('at the exchange');
  });
  it('reads the month from the CAIDA file name', () => {
    expect(asrelMonth('20261001.as-rel2.txt.bz2')).toBe('October 2026');
    expect(asrelMonth('junk')).toBeNull();
  });
});

describe('clouds', () => {
  const e = (from: number, to: number, kind: NetLink['kind'] = 'direct'): NetLink => ({ ...l(to, kind), from, handoff: false });
  const target = (id: string, cdn: string | null) => ({ target: { id, cdn, name: cdn?.toUpperCase() ?? id }, trace: {}, same_route: null });
  const run = (groups: NetGroup[], edges: NetLink[]) =>
    ({ groups, edges, traces: [target('a1', 'a'), target('a2', 'a'), target('b1', 'b'), target('dns', null)] }) as unknown as MapRun;
  // Home, an ISP, an exchange with two destinations, and a carrier with one.
  const groups = [g(0, 'home'), g(1, 'isp'), g(2, 'exchange'), g(3, 'content', { targets: ['a1', 'dns'] }), g(4, 'carrier'), g(5, 'content', { targets: ['a2'] }), g(6, 'content', { targets: ['b1'] })];
  it('finds a CDN reached at the exchange and over a carrier, and not one only at the exchange', () => {
    expect(clouds(run(groups, [e(0, 1, 'unknown'), e(1, 2, 'exchange'), e(2, 3), e(1, 4, 'via_transit'), e(4, 5), e(2, 6)]))).toEqual([{ cdn: 'a', name: 'A', ids: [3, 5] }]);
  });
  it("counts your ISP's own link as past the exchange", () => {
    expect(clouds(run(groups, [e(0, 1, 'unknown'), e(1, 2, 'exchange'), e(2, 3), e(1, 5), e(2, 6)]))).toHaveLength(1);
  });
  it('finds one box reached both ways', () => {
    expect(clouds(run(groups.filter((x) => x.id !== 5), [e(0, 1, 'unknown'), e(1, 2, 'exchange'), e(2, 3), e(1, 4, 'via_transit'), e(4, 3)]))).toEqual([{ cdn: 'a', name: 'A', ids: [3] }]);
  });
  it('finds nothing when every box is past the exchange, or there is none', () => {
    expect(clouds(run(groups, [e(0, 1, 'unknown'), e(1, 3), e(1, 4, 'via_transit'), e(4, 5)]))).toEqual([]);
    expect(clouds(run([], []))).toEqual([]);
  });
});
