import { describe, it, expect } from 'vitest';
import { shortcut, shortcutParts } from './keys';

describe('shortcut', () => {
  it('prints macOS glyphs in Apple order, without separators', () => {
    expect(shortcut('cmd+N', true)).toBe('⌘N');
    expect(shortcut('alt+cmd+1', true)).toBe('⌥⌘1');
    expect(shortcut('cmd+ctrl+S', true)).toBe('⌃⌘S');
    expect(shortcut('cmd+Enter', true)).toBe('⌘↵');
    expect(shortcut('cmd+,', true)).toBe('⌘,');
  });
  it('spells modifiers out elsewhere: cmd is Ctrl, alt is Shift, ctrl is Alt', () => {
    expect(shortcut('cmd+N', false)).toBe('Ctrl+N');
    expect(shortcut('alt+cmd+1', false)).toBe('Ctrl+Shift+1');
    expect(shortcut('ctrl+cmd+S', false)).toBe('Ctrl+Alt+S');
    expect(shortcut('cmd+Enter', false)).toBe('Ctrl+Enter');
    expect(shortcut('alt+shift+cmd+X', false)).toBe('Ctrl+Shift+X');
  });
  it('gives the parts for separate kbd elements', () => {
    expect(shortcutParts('cmd+Enter', true)).toEqual(['⌘', '↵']);
    expect(shortcutParts('cmd+Enter', false)).toEqual(['Ctrl', 'Enter']);
  });
});
