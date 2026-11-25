import { describe, it, expect } from 'vitest';
import { isPrivate, monitoredCount, resolverName } from './format';
import type { Resolver } from './types';

const r = (id: string, extra: Partial<Resolver> = {}) => ({ id, ...extra }) as Resolver;

describe('monitoredCount', () => {
  it('leaves out ids the resolver list no longer has', () => {
    expect(monitoredCount([r('one'), r('two')], ['one', 'two', 'gone'])).toBe(2);
  });
  it('counts a folded custom entry as its row, once', () => {
    expect(monitoredCount([r('one', { custom_id: 'custom-1', aliases: ['custom-2'] })], ['custom-1', 'custom-2'])).toBe(1);
  });
});

describe('resolverName', () => {
  it('names a resolver that has left the list, and falls back to the id', () => {
    expect(resolverName([], 'cleanbrowsing-family')).toBe('CleanBrowsing Family');
    expect(resolverName([], 'mullvad')).toBe('Mullvad Unfiltered');
    expect(resolverName([], 'adguard')).toBe('AdGuard Default');
    expect(resolverName([], 'custom-9')).toBe('custom-9');
  });
});

describe('isPrivate', () => {
  it('is true for on-network addresses and false for public ones', () => {
    for (const ip of ['192.168.1.1', '10.0.0.53', '172.16.0.1', 'fd00::1', 'feb0::1']) expect(isPrivate(ip)).toBe(true);
    for (const ip of ['192.0.2.1', '2001:db8::1']) expect(isPrivate(ip)).toBe(false);
  });
});
