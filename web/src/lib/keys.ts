// Shortcut labels for tooltips and hints: the macOS glyphs on a Mac ("⌥⌘1"), "Ctrl+Shift+1" elsewhere.
// A spec is written in macOS terms: 'cmd+N', 'alt+cmd+1', 'ctrl+cmd+S', 'cmd+Enter'. Off a Mac, cmd becomes Ctrl,
// alt becomes Shift (Windows reports AltGr as Ctrl+Alt, so Ctrl+Alt+key would steal characters), ctrl becomes Alt.

type Nav = Navigator & { userAgentData?: { platform?: string } };
const platform = (navigator as Nav).userAgentData?.platform || navigator.platform || '';

/** The keyboard has a Command key. */
export const isMac = /^mac|^i(phone|pad|pod)/i.test(platform);

type Mod = 'ctrl' | 'alt' | 'shift' | 'cmd';
const MAC: Record<Mod, string> = { ctrl: '⌃', alt: '⌥', shift: '⇧', cmd: '⌘' };
const OTHER: Record<Mod, string> = { cmd: 'Ctrl', ctrl: 'Alt', alt: 'Shift', shift: 'Shift' };
const MAC_KEYS: Record<string, string> = { Enter: '↵', Escape: '⎋', Backspace: '⌫', Delete: '⌦', Tab: '⇥' };

/** The parts of a shortcut, for separate `<kbd>` elements. The last one is the key. */
export function shortcutParts(spec: string, mac = isMac): string[] {
  const parts = spec.split('+');
  const key = parts.pop() ?? '';
  const mods = parts as Mod[];
  if (mac) {
    const order: Mod[] = ['ctrl', 'alt', 'shift', 'cmd'];
    return [...order.filter((m) => mods.includes(m)).map((m) => MAC[m]), MAC_KEYS[key] ?? key];
  }
  const order = ['Ctrl', 'Alt', 'Shift'];
  const names = new Set(mods.map((m) => OTHER[m]));
  return [...order.filter((n) => names.has(n)), key];
}

/** One label: "⌘N" on a Mac, "Ctrl+N" elsewhere. */
export function shortcut(spec: string, mac = isMac): string {
  return shortcutParts(spec, mac).join(mac ? '' : '+');
}
