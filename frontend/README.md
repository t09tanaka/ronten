# ronten frontend

The Svelte review UI for [ronten](../README.md). Built to static assets and embedded into
the `ronten` binary at compile time (see `../build.rs`); not served standalone in
production.

## Dev commands

```sh
npm install
npm run dev      # Vite dev server; proxies /api to a running ronten process
npm run check    # svelte-check + tsc
npm run test     # vitest
npm run build    # production build into frontend/dist
```

For the full frontend development loop (running a backend session to develop against), see
the root [README's Development section](../README.md#development).
