// Context and pull-down menus: native in the desktop app, a small HTML menu in the browser.

import { nativeMenu, type MenuItem } from './api';

interface Open {
  x: number;
  y: number;
  items: MenuItem[];
  pick: (id: string) => void;
}

class MenuState {
  open = $state<Open | null>(null);

  /** Show `items` at the pointer (or under `anchor`); `pick` gets the chosen id. */
  show(e: MouseEvent, items: MenuItem[], pick: (id: string) => void, anchor?: HTMLElement) {
    e.preventDefault();
    e.stopPropagation();
    if (nativeMenu(items, pick)) return;
    const r = anchor?.getBoundingClientRect();
    this.open = { x: r ? r.left : e.clientX, y: r ? r.bottom + 4 : e.clientY, items, pick };
  }

  close() {
    this.open = null;
  }
}

export const menu = new MenuState();
