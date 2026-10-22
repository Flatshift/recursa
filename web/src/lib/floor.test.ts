import { describe, expect, it } from 'vitest';
import { sentence, split } from './floor';

describe('split', () => {
  it('divides the medians into wire, resolver and recursion', () => {
    const s = split(7.2, 10.3, 14.5)!;
    expect(s.resolver).toBeCloseTo(3.1);
    expect(s.recursion).toBeCloseTo(4.2);
  });
  it('needs a floor', () => {
    expect(split(null, 10, 14)).toBeNull();
    expect(split(NaN, 10, 14)).toBeNull();
  });
  it('measures recursion from the wire when no cached test ran', () => {
    const s = split(7.2, null, 14.5)!;
    expect(s.resolver).toBeNull();
    expect(s.recursion).toBeCloseTo(7.3);
  });
  it('keeps a negative resolver share when answers beat the floor', () => {
    expect(split(7.2, 2.0, null)!.resolver).toBeCloseTo(-5.2);
  });
});

describe('sentence', () => {
  it('reads the three parts', () => {
    expect(sentence('Cloudflare', split(7.2, 10.3, 14.5)!)).toBe('Cloudflare adds 3.1 ms to a 7.2 ms wire for cached answers; recursion adds 4.2 ms more.');
  });
  it('handles a missing test', () => {
    expect(sentence('Cloudflare', split(7.2, null, 14.5)!)).toBe('Cloudflare adds 7.3 ms to a 7.2 ms wire for uncached answers.');
    expect(sentence('Cloudflare', split(7.2, 10.3, null)!)).toBe('Cloudflare adds 3.1 ms to a 7.2 ms wire for cached answers.');
    expect(sentence('Cloudflare', split(7.2, null, null)!)).toBe('The wire to Cloudflare is 7.2 ms.');
  });
  it('names interception when answers beat the wire', () => {
    expect(sentence('Google', split(7.2, 2.0, 9.0)!)).toBe("Google's cached answers arrived 5.2 ms under the 7.2 ms wire: something nearer answered them.");
  });
});
