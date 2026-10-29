// Tidy-tree layout for the network map: the groups become columns by path order, from your home on the left to the
// destinations on the right, with one row per leaf and every parent centred on its children. A group several traces
// reach from different sides hangs under the parent that carries most of them; its other links are still drawn.
import type { NetGroup, NetLink } from './types';

export interface TreeOpts {
  colW: number;
  rowH: number;
  nodeW: number;
  nodeH: number;
  pad: number;
}
export const TREE: TreeOpts = { colW: 292, rowH: 72, nodeW: 188, nodeH: 50, pad: 16 };
/** Half the width of the diamond an exchange is drawn as, centred in its node; its edges end on the points. */
export const DIAMOND = 13;

export interface TreeNode {
  id: number;
  col: number;
  /** The top-left corner. */
  x: number;
  y: number;
  w: number;
  h: number;
}
export interface TreeEdge {
  /** The index in `edges`. */
  index: number;
  d: string;
  /** Where the label sits: centred on `lx`, stacked up from `ly` when `above`, else down from it. */
  lx: number;
  ly: number;
  above: boolean;
}
export interface Tree {
  nodes: TreeNode[];
  edges: TreeEdge[];
  width: number;
  height: number;
}

export function layout(groups: NetGroup[], edges: NetLink[], o: TreeOpts = TREE): Tree {
  const ids = new Set(groups.map((g) => g.id));
  const real = edges.filter((e) => e.from !== e.to && ids.has(e.from) && ids.has(e.to));

  // Depth is the longest path from a root; the pass count stops a cycle from looping.
  const col = new Map<number, number>(groups.map((g) => [g.id, 0]));
  for (let i = 0; i < groups.length; i++) {
    let moved = false;
    for (const e of real) {
      const c = col.get(e.from)! + 1;
      if (c > col.get(e.to)! && c <= groups.length) {
        col.set(e.to, c);
        moved = true;
      }
    }
    if (!moved) break;
  }

  // Each group hangs under the parent that carries most traces.
  const parent = new Map<number, number>();
  const weight = new Map<number, number>();
  for (const e of real) {
    if (col.get(e.from)! >= col.get(e.to)!) continue;
    if (e.count > (weight.get(e.to) ?? 0)) {
      weight.set(e.to, e.count);
      parent.set(e.to, e.from);
    }
  }
  const order = [...groups].sort((a, b) => a.ttl_from - b.ttl_from || a.id - b.id);
  const kids = new Map<number, number[]>();
  for (const g of order) {
    const p = parent.get(g.id);
    if (p != null) kids.set(p, [...(kids.get(p) ?? []), g.id]);
  }

  const y = new Map<number, number>();
  let row = 0;
  const place = (id: number): number => {
    const ks = kids.get(id) ?? [];
    const v = ks.length ? ks.reduce((s, k) => s + place(k), 0) / ks.length : row++;
    y.set(id, v);
    return v;
  };
  // The home group first, then anything with no parent in ttl order.
  for (const g of [...order].sort((a, b) => Number(b.role === 'home') - Number(a.role === 'home'))) {
    if (!parent.has(g.id)) place(g.id);
  }

  const nodes = groups.map((g): TreeNode => {
    const c = col.get(g.id)!;
    return { id: g.id, col: c, x: o.pad + c * o.colW, y: o.pad + (y.get(g.id) ?? 0) * o.rowH, w: o.nodeW, h: o.nodeH };
  });
  const at = new Map(nodes.map((n) => [n.id, n]));
  const diamond = new Set(groups.filter((g) => g.role === 'exchange').map((g) => g.id));
  // Where an edge meets a node: the sides of its box, or the points of an exchange's diamond.
  const side = (n: TreeNode, right: boolean) => (diamond.has(n.id) ? n.x + n.w / 2 + (right ? DIAMOND : -DIAMOND) : right ? n.x + n.w : n.x);
  // A label sits in the gap before its destination, where the curve has flattened to that row and labels of the
  // edges fanning out of one node are a row apart. It goes on the side the curve does not come in from (above a
  // diamond, whose name is below it); a second edge into the same node takes the other side.
  const into = new Map<number, number>();
  const paths = edges.map((e, index): TreeEdge | null => {
    const a = at.get(e.from);
    const b = at.get(e.to);
    if (!a || !b) return null;
    const [x1, y1, x2, y2] = [side(a, true), a.y + a.h / 2, side(b, false), b.y + b.h / 2];
    const xm = (x1 + x2) / 2;
    const nth = into.get(e.to) ?? 0;
    into.set(e.to, nth + 1);
    const above = (diamond.has(e.to) || y1 >= y2) !== (nth % 2 === 1);
    return { index, d: `M${x1} ${y1}C${xm} ${y1} ${xm} ${y2} ${x2} ${y2}`, lx: x2 - (o.colW - o.nodeW) / 2, ly: y2, above };
  });
  return {
    nodes,
    edges: paths.filter((p): p is TreeEdge => p != null),
    width: o.pad * 2 + Math.max(0, ...nodes.map((n) => n.col)) * o.colW + o.nodeW,
    height: o.pad * 2 + Math.max(row - 1, 0) * o.rowH + o.nodeH,
  };
}
