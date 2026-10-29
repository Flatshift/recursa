import { describe, expect, it } from 'vitest';
import { DIAMOND, layout, TREE } from './tree';
import type { NetGroup, NetLink, NetRole } from './types';

const g = (id: number, role: NetRole, ttl: number): NetGroup => ({
  id,
  role,
  label: `g${id}`,
  asn: null,
  holder: null,
  owner: null,
  ix_id: null,
  ix_name: null,
  ttl_from: ttl,
  ttl_to: ttl,
  parallel: false,
  unnumbered: false,
  city_hint: null,
  rtt_ms: null,
  members: [],
  aliases: [],
  targets: [],
});
const l = (from: number, to: number, count = 1, handoff = false): NetLink => ({ from, to, count, targets: [], rtt_step_ms: 1, kind: 'direct', handoff, silent_hops: 0 });

// Home, an ISP, then an exchange with two destinations, a carrier with one and a silent stretch with one.
const groups = [
  g(0, 'home', 1),
  g(1, 'isp', 2),
  g(2, 'exchange', 4),
  g(3, 'content', 5),
  g(4, 'content', 5),
  g(5, 'carrier', 4),
  g(6, 'content', 6),
  g(7, 'silent', 4),
  g(8, 'content', 8),
];
const edges = [l(0, 1), l(1, 2, 2, true), l(2, 3), l(2, 4), l(1, 5, 1, true), l(5, 6), l(1, 7, 1, true), l(7, 8), l(5, 4)];

describe('layout', () => {
  const t = layout(groups, edges);
  it('puts a node in each group, in columns by path order', () => {
    expect(t.nodes).toHaveLength(groups.length);
    const col = (id: number) => t.nodes.find((n) => n.id === id)!.col;
    expect([col(0), col(1), col(2), col(3)]).toEqual([0, 1, 2, 3]);
  });
  it('leaves the same gap after the home square as between the other columns', () => {
    const n = (id: number) => t.nodes.find((x) => x.id === id)!;
    expect(n(1).x - (n(0).x + n(0).w)).toBe(n(2).x - (n(1).x + n(1).w));
  });
  it('overlaps no two nodes', () => {
    for (const a of t.nodes)
      for (const b of t.nodes) {
        if (a.id >= b.id) continue;
        const apart = a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y;
        expect(apart, `${a.id} and ${b.id}`).toBe(true);
      }
  });
  it('draws every edge and fits the nodes', () => {
    expect(t.edges).toHaveLength(edges.length);
    for (const n of t.nodes) {
      expect(n.x + n.w).toBeLessThanOrEqual(t.width);
      expect(n.y + n.h).toBeLessThanOrEqual(t.height);
    }
  });
  it('ends an edge on the points of an exchange diamond, not its box', () => {
    const n = (id: number) => t.nodes.find((x) => x.id === id)!;
    const into = t.edges.find((e) => edges[e.index].to === 2)!;
    const outOf = t.edges.find((e) => edges[e.index].from === 2)!;
    expect(into.d.endsWith(`${n(2).x + n(2).w / 2 - DIAMOND} ${n(2).y + n(2).h / 2}`)).toBe(true);
    expect(outOf.d.startsWith(`M${n(2).x + n(2).w / 2 + DIAMOND} `)).toBe(true);
    const plain = t.edges.find((e) => edges[e.index].to === 5)!;
    expect(plain.d.endsWith(`${n(5).x} ${n(5).y + n(5).h / 2}`)).toBe(true);
  });
  it('pulls groups asked to sit together onto adjacent rows, keeping the height', () => {
    const u = layout(groups, edges, undefined, [[3, 6]]);
    const row = (id: number) => u.nodes.find((n) => n.id === id)!.y;
    expect(row(6) - row(3)).toBe(TREE.rowH);
    expect(u.height).toBe(t.height);
    expect(row(5)).toBe(row(6));
  });
  it('survives a cycle', () => {
    expect(layout(groups.slice(0, 3), [l(0, 1), l(1, 2), l(2, 1)]).nodes).toHaveLength(3);
  });
  it('lays out nothing as nothing', () => {
    expect(layout([], [])).toMatchObject({ nodes: [], edges: [] });
  });
});
