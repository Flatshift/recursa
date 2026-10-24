import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// The UI is embedded into the Rust binary from web/dist. During development,
// `npm run dev` proxies API calls to a running `recursa --no-open`. The proxy presents them as
// coming from Recursa's own address, since the server refuses any other Host or Origin.
const api = 'http://127.0.0.1:7353';
export default defineConfig({
  plugins: [svelte()],
  build: { target: 'es2022', cssCodeSplit: false, assetsInlineLimit: 0, sourcemap: false },
  server: { proxy: { '/api': { target: api, changeOrigin: true, headers: { origin: api } } } },
});
