// Emits the "bundled frontend assets" half of THIRD_PARTY_NOTICES.md.
// Invoked by scripts/gen-third-party.sh; writes markdown to stdout.
//
// cargo-about covers the Rust dependency tree, but the binary also embeds
// frontend/dist — compiled JS/CSS and a woff2 font — whose third-party
// content npm's dependency graph does not describe accurately: `svelte` is a
// devDependency yet its runtime is compiled into the bundle, while most other
// devDependencies (vite, vitest, svelte-check, ...) never ship. So the set of
// components that actually reach a user is listed explicitly below and the
// versions/license texts are read from the lockfile and node_modules.

import { readFileSync, existsSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const frontend = join(root, 'frontend')

// Every third-party component present in a built frontend/dist.
const BUNDLED = [
  {
    name: 'svelte',
    what: 'Client-side runtime compiled into the application bundle.',
    licenseFiles: ['LICENSE.md', 'LICENSE'],
  },
  {
    name: 'highlight.js',
    what: 'Syntax highlighting for diff content.',
    licenseFiles: ['LICENSE'],
  },
]

// Vendored assets that are not npm packages at all.
const VENDORED = [
  {
    name: 'Shippori Mincho',
    version: 'subset of the Google Fonts release',
    license: 'OFL-1.1',
    what: 'Display typeface (frontend/src/assets/fonts/*.woff2, subset).',
    licensePath: join(frontend, 'public', 'shippori-mincho-OFL.txt'),
  },
]

function fail(message) {
  console.error(`js-notices: ${message}`)
  process.exit(1)
}

const modules = join(frontend, 'node_modules')
if (!existsSync(modules)) {
  fail('frontend/node_modules is missing — run `npm ci` in frontend/ first')
}

const lock = JSON.parse(readFileSync(join(frontend, 'package-lock.json'), 'utf8'))
const pkg = JSON.parse(readFileSync(join(frontend, 'package.json'), 'utf8'))

// A new runtime dependency is bundled by definition, so it must be declared
// above rather than silently omitted from the notices.
const listed = new Set(BUNDLED.map((b) => b.name))
for (const name of Object.keys(pkg.dependencies ?? {})) {
  if (!listed.has(name)) {
    fail(`frontend dependency "${name}" is bundled but missing from BUNDLED in this script`)
  }
}

const out = []
for (const entry of BUNDLED) {
  const locked = lock.packages?.[`node_modules/${entry.name}`]
  if (!locked) fail(`${entry.name} is not present in frontend/package-lock.json`)

  const file = entry.licenseFiles.find((f) => existsSync(join(modules, entry.name, f)))
  if (!file) {
    fail(`no license file found for ${entry.name} (looked for ${entry.licenseFiles.join(', ')})`)
  }
  const text = readFileSync(join(modules, entry.name, file), 'utf8').trimEnd()

  out.push(`### ${entry.name} ${locked.version} (${locked.license ?? 'see text below'})`)
  out.push('')
  out.push(entry.what)
  out.push('')
  out.push('```text')
  out.push(text)
  out.push('```')
  out.push('')
}

for (const entry of VENDORED) {
  const text = readFileSync(entry.licensePath, 'utf8').trimEnd()
  out.push(`### ${entry.name} — ${entry.version} (${entry.license})`)
  out.push('')
  out.push(entry.what)
  out.push('')
  out.push('```text')
  out.push(text)
  out.push('```')
  out.push('')
}

process.stdout.write(out.join('\n'))
