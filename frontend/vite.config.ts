import { defineConfig } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

export default defineConfig({
  plugins: [svelte()],
  server: {
    proxy: { '/api': process.env.RONTEN_DEV_API ?? 'http://127.0.0.1:8877' },
  },
  // Under Vitest (component-mount tests, e.g. DiffView.mount.test.ts),
  // resolve Svelte's "browser" build instead of its default SSR/server
  // build — otherwise `mount()` throws lifecycle_function_unavailable,
  // since the server build has no client lifecycle to mount into. Left
  // undefined outside Vitest so normal dev/build resolution is untouched.
  resolve: process.env.VITEST ? { conditions: ['browser'] } : undefined,
})
