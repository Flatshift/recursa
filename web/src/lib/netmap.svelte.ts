// The network map's controller: the last run, a poll every two seconds while one is running, and the actions.
// The page and its toolbar share one, so the toolbar can show progress that the page is drawing.

import { api } from './api';
import { app } from './store.svelte';
import type { MapResp, NetDatasets } from './types';

export function netmap() {
  let data = $state<MapResp | null>(null);
  let loaded = $state(false);
  let error = $state('');
  let busy = $state(false);

  async function load() {
    try {
      data = await api.get<MapResp>('/api/network/map');
      error = data.error ?? '';
    } catch (e) {
      error = (e as Error).message;
    } finally {
      loaded = true;
    }
  }

  $effect(() => {
    load();
  });
  // Poll only while a run is going; the run's last answer lands in `last` on the tick that sees it end.
  $effect(() => {
    if (!data?.running) return;
    const t = setInterval(() => {
      if (!document.hidden) load();
    }, 2000);
    return () => clearInterval(t);
  });

  async function start() {
    if (busy || data?.running) return;
    busy = true;
    error = '';
    try {
      await api.post('/api/network/map');
    } catch (e) {
      // A 409 says why (a benchmark is running, say), in the engine's words.
      error = (e as Error).message;
    } finally {
      busy = false;
    }
    await load();
  }

  async function cancel() {
    try {
      await api.del('/api/network/map');
    } catch (e) {
      error = (e as Error).message;
    }
    await load();
  }

  /** A dataset setting changed: the server answers with the datasets as they now stand. */
  async function set(path: 'peeringdb' | 'rdns' | 'rdap', body: { accepted: boolean } | { on: boolean }) {
    try {
      const datasets = await api.put<NetDatasets>(`/api/network/map/${path}`, body);
      if (data) data = { ...data, datasets };
    } catch (e) {
      app.notify((e as Error).message, 'error');
    }
  }

  return {
    get data() {
      return data;
    },
    get loaded() {
      return loaded;
    },
    get error() {
      return error;
    },
    get busy() {
      return busy;
    },
    get running() {
      return !!data?.running || busy;
    },
    load,
    start,
    cancel,
    allowPeeringDb: (accepted = true) => set('peeringdb', { accepted }),
    allowRegistry: (accepted = true) => set('rdap', { accepted }),
    decodeNames: (on: boolean) => set('rdns', { on }),
  };
}
