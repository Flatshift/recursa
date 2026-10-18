import type { CheckStatus, Resolver, RunState } from './types';
import { CAP } from './caps';
import { check } from './verdict';
import { holder, ms, pct, when } from './format';

export interface Cell {
  status: CheckStatus;
  text: string;
  prov: 'verified' | 'published' | 'unknown';
  evidence?: string;
}

/** A capability value: measured beats published beats unknown. */
export function capCell(run: RunState | null, r: Resolver, key: string): Cell {
  let c = run ? check(run, r.id, key) : undefined;
  // Runs saved before the engine stopped reporting "no endpoint" as a measurement.
  if (c?.value === 'Not published') c = { ...c, status: 'unknown', value: r.group === 'system' || r.group === 'custom' ? 'Not set' : 'Not offered' };
  if (c && c.status !== 'unknown') {
    // Not blocking isn't a failure for a resolver that never claimed to filter.
    if (key.startsWith('filter_') && c.status === 'fail' && !r.published.filtering.includes(key.slice(7))) {
      return { status: 'info', text: `None · ${c.value.replace(' blocked', '')}`, prov: 'verified', evidence: c.evidence };
    }
    return { status: c.status, text: c.value, prov: 'verified', evidence: c.evidence };
  }
  // A probed resolver whose blocking category this run didn't test: say so, rather than fall back to a claim or a blank.
  if (key.startsWith('filter_') && run?.probes[r.id] && !run.probes[r.id].filtering?.[key.slice(7)]) {
    return { status: 'unknown', text: 'Not tested', prov: 'unknown', evidence: c?.evidence || 'Not in this run’s test plan' };
  }
  const def = CAP[key];
  // Only catalog entries have operator documentation. For the system resolver and custom
  // entries, anything not measured is unknown, never an implied "no".
  const documented = r.group === 'public' || r.group === 'legacy';
  const pub = documented ? def?.published?.(r.published, r) : null;
  if (pub != null) {
    const status: CheckStatus = def.goodWhenPresent === undefined ? 'info' : pub === def.goodWhenPresent ? 'pass' : 'fail';
    const filterKey = key.startsWith('filter_');
    return {
      status: filterKey ? 'info' : status,
      text: filterKey ? (pub ? 'Claims to block' : 'No blocking') : pub ? 'Yes' : 'No',
      prov: 'published',
      evidence: 'From the operator’s documentation',
    };
  }
  if (c) return { status: 'unknown', text: c.value, prov: 'unknown', evidence: c.evidence };
  return { status: 'unknown', text: '—', prov: 'unknown' };
}

export function roaCell(run: RunState, id: string): Cell {
  const s = run.intel[id]?.service;
  if (!s) return { status: 'unknown', text: '—', prov: 'unknown' };
  if (s.private) return { status: 'info', text: 'Private address', prov: 'verified' };
  const st = s.rpki?.status;
  if (!st) return { status: 'unknown', text: 'No data', prov: 'unknown', evidence: s.error ?? 'No route-origin result for this address' };
  if (st === 'not_checked') {
    return { status: 'unknown', text: s.rpki?.state === 'syncing' ? 'Syncing…' : 'Not checked', prov: 'unknown', evidence: s.rpki?.reason };
  }
  const roa = s.rpki?.roas?.[0];
  const ev = `${s.prefix ?? ''} origin AS${s.asn ?? '?'} · ${roa ? `ROA AS${roa.origin} ${roa.prefix} max /${roa.max_length}` : 'no covering ROA'} (${s.rpki?.vrp_time != null ? `checked on this computer, data from ${when(s.rpki.vrp_time)}; ${s.rpki.validator ?? 'RPKI validator'}` : 'route-origin result from an earlier version'})`;
  if (st === 'valid') return { status: 'pass', text: 'Valid', prov: 'verified', evidence: ev };
  if (st.startsWith('invalid')) return { status: 'fail', text: 'Invalid', prov: 'verified', evidence: ev };
  return { status: 'partial', text: 'No ROA', prov: 'verified', evidence: ev };
}

export function rovCell(run: RunState, id: string): Cell {
  const s = run.intel[id]?.service;
  if (!s || s.private) return { status: 'unknown', text: '—', prov: 'unknown' };
  const r = s.rov;
  if (!r) return { status: 'unknown', text: 'No data', prov: 'unknown', evidence: `AS${s.asn ?? '?'} isn’t in RoVista’s dataset` };
  const status: CheckStatus = r.ratio >= 0.9 ? 'pass' : r.ratio > 0.1 ? 'partial' : 'fail';
  return {
    status,
    text: pct(r.ratio, 0),
    prov: 'verified',
    evidence: `RoVista: ${pct(r.ratio, 0)} of RPKI-invalid routes are filtered from AS${s.asn}'s point of view (updated ${r.updated})`,
  };
}

export function egressCell(run: RunState, id: string): Cell {
  const e = run.intel[id]?.egress;
  const p = run.probes[id];
  if (!e) return { status: 'unknown', text: p?.egress[0] ?? '—', prov: p?.egress.length ? 'verified' : 'unknown' };
  return {
    status: 'info',
    text: holder(e.holder) || e.ip,
    prov: 'verified',
    evidence: `${e.ip} · AS${e.asn ?? '?'}`,
  };
}

export function cdnCell(run: RunState, id: string): Cell {
  const c = run.cdn?.resolvers[id];
  if (!c || c.penalty_ms == null) return { status: 'unknown', text: '—', prov: 'unknown' };
  const p = c.penalty_ms;
  const status: CheckStatus = p <= 2 ? 'pass' : p <= 10 ? 'partial' : 'fail';
  return { status, text: p < 0.05 ? 'best' : `+${ms(p, false)} ms`, prov: 'verified', evidence: `Extra round trip to CDN edges vs the best seen, averaged over the CDNs` };
}
