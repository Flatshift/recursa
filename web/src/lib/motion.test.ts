import { describe, expect, it } from 'vitest';
import { jitterOf, sinFrac } from './motion';
import type { Sample, TestKind } from './types';
import { warmJitter } from './warm';

const sample = (t: number, i: number, k: TestKind): Sample => ({ t, i, k, us: 1000, o: 'ok', at: 0, h: 0 });

// Pinned from the hash as it was written out in each view, so the placement of dots, rings and ghosts stays put.
describe('sinFrac', () => {
  it('is the fraction of a scaled sine', () => {
    expect(sinFrac(12.9898)).toBe(0.9216903898159217);
    expect(sinFrac(77.9388)).toBe(0.626086772987037);
    expect(sinFrac(299.648)).toBe(0.9064690540690208);
    expect(sinFrac(586.3062)).toBe(0.41706695510220015);
  });
  it('takes the multiplier', () => {
    expect(sinFrac(406.26, 24634.6345)).toBe(0.5487084472333663);
    expect(sinFrac(816.34, 24634.6345)).toBe(0.576368986048692);
    expect(sinFrac(2186.91, 24634.6345)).toBe(0.022308664070806117);
  });
});

describe('jitterOf', () => {
  it('places a dot by its target, index and test', () => {
    expect(jitterOf(sample(0, 0, 'cached'))).toBe(-0.12463258554089407);
    expect(jitterOf(sample(3, 7, 'uncached'))).toBe(-0.6182320048756083);
    expect(jitterOf(sample(12, 41, 'tld'))).toBe(0.6905934405367589);
  });
});

describe('warmJitter', () => {
  it('places a warm-up ring by its target, test and index', () => {
    expect(warmJitter(0, 'cached', 0)).toBe(-0.9547647957151639);
    expect(warmJitter(3, 'uncached', 2)).toBe(0.5055564944486832);
    expect(warmJitter(12, 'tld', 9)).toBe(-0.47224733195616864);
  });
});
