import { describe, it, expect } from 'vitest';
import { rovCell } from './cells';
import type { RovCheck, RunState } from './types';

const chk = (status: RovCheck['status'], detail = 'd'): RovCheck => ({ status, detail, beacon: 'cmdns3-sjc.dns-oarc.net' });
const runWith = (rov?: { v4: RovCheck; v6: RovCheck }): RunState => ({ intel: rov ? { r: { service: null, egress: null, rov } } : {} }) as unknown as RunState;

describe('rovCell', () => {
  it('is blank with no intel', () => {
    expect(rovCell(runWith(), 'r')).toMatchObject({ status: 'unknown', text: '—', prov: 'unknown' });
  });

  it('passes when both families are filtered', () => {
    const c = rovCell(runWith({ v4: chk('filtered'), v6: chk('filtered') }), 'r');
    expect(c).toMatchObject({ status: 'pass', text: 'IPv4 + IPv6', prov: 'verified' });
    expect(c.evidence).toBe('Check My DNS by DNS-OARC · IPv4: d, cmdns3-sjc.dns-oarc.net · IPv6: d, cmdns3-sjc.dns-oarc.net');
  });

  it('judges only the family that was tested', () => {
    const c = rovCell(runWith({ v4: chk('filtered'), v6: chk('not_tested', 'never left') }), 'r');
    expect(c).toMatchObject({ status: 'pass', text: 'IPv4', prov: 'verified' });
    expect(c.evidence).toContain('IPv6: never left');
  });

  it('is not tested when neither family was', () => {
    expect(rovCell(runWith({ v4: chk('not_tested'), v6: chk('not_tested') }), 'r')).toMatchObject({ status: 'unknown', text: 'Not tested', prov: 'unknown' });
  });

  it('fails when both families are unfiltered', () => {
    expect(rovCell(runWith({ v4: chk('unfiltered'), v6: chk('unfiltered') }), 'r')).toMatchObject({ status: 'fail', text: 'No', prov: 'verified' });
  });

  it('is partial when only one family is filtered', () => {
    expect(rovCell(runWith({ v4: chk('unfiltered'), v6: chk('filtered') }), 'r')).toMatchObject({ status: 'partial', text: 'IPv6 only' });
    expect(rovCell(runWith({ v4: chk('filtered'), v6: chk('unfiltered') }), 'r')).toMatchObject({ status: 'partial', text: 'IPv4 only' });
  });
});
