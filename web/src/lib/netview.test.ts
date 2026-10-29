import { describe, expect, it } from 'vitest';
import { asrelMonth, linkLabel } from './netview';
import type { NetGroup, NetLink } from './types';

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
