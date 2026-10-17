import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// The UI is embedded into the Rust binary from web/dist. During development,
// `npm run dev` proxies API calls to a running `recursa --no-open`.
export default defineConfig({
  plugins: [svelte()],
  build: { target: 'es2022', cssCodeSplit: false, assetsInlineLimit: 0, sourcemap: false },
  server: { proxy: { '/api': 'http://127.0.0.1:7353' } },
});
