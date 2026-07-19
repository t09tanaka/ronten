// Per-line syntax highlighting for the diff view. Registers a curated set of
// hljs grammars (tree-shaken by Vite) instead of the full bundle to keep the
// embedded binary small. Highlighting is line-by-line, so multi-line
// constructs (block comments, template strings) degrade to plain text past
// their first line — the same trade-off GitHub's diff view makes.
import hljs from 'highlight.js/lib/core'
import bash from 'highlight.js/lib/languages/bash'
import c from 'highlight.js/lib/languages/c'
import cpp from 'highlight.js/lib/languages/cpp'
import css from 'highlight.js/lib/languages/css'
import go from 'highlight.js/lib/languages/go'
import ini from 'highlight.js/lib/languages/ini'
import java from 'highlight.js/lib/languages/java'
import javascript from 'highlight.js/lib/languages/javascript'
import json from 'highlight.js/lib/languages/json'
import markdown from 'highlight.js/lib/languages/markdown'
import python from 'highlight.js/lib/languages/python'
import rust from 'highlight.js/lib/languages/rust'
import scss from 'highlight.js/lib/languages/scss'
import sql from 'highlight.js/lib/languages/sql'
import typescript from 'highlight.js/lib/languages/typescript'
import xml from 'highlight.js/lib/languages/xml'
import yaml from 'highlight.js/lib/languages/yaml'

hljs.registerLanguage('bash', bash)
hljs.registerLanguage('c', c)
hljs.registerLanguage('cpp', cpp)
hljs.registerLanguage('css', css)
hljs.registerLanguage('go', go)
hljs.registerLanguage('ini', ini)
hljs.registerLanguage('java', java)
hljs.registerLanguage('javascript', javascript)
hljs.registerLanguage('json', json)
hljs.registerLanguage('markdown', markdown)
hljs.registerLanguage('python', python)
hljs.registerLanguage('rust', rust)
hljs.registerLanguage('scss', scss)
hljs.registerLanguage('sql', sql)
hljs.registerLanguage('typescript', typescript)
hljs.registerLanguage('xml', xml)
hljs.registerLanguage('yaml', yaml)

const EXT_TO_LANG: Record<string, string> = {
  bash: 'bash',
  sh: 'bash',
  zsh: 'bash',
  c: 'c',
  h: 'c',
  cc: 'cpp',
  cpp: 'cpp',
  cxx: 'cpp',
  hpp: 'cpp',
  css: 'css',
  go: 'go',
  ini: 'ini',
  toml: 'ini',
  java: 'java',
  cjs: 'javascript',
  js: 'javascript',
  jsx: 'javascript',
  mjs: 'javascript',
  json: 'json',
  markdown: 'markdown',
  md: 'markdown',
  py: 'python',
  rs: 'rust',
  scss: 'scss',
  sql: 'sql',
  ts: 'typescript',
  tsx: 'typescript',
  htm: 'xml',
  html: 'xml',
  svelte: 'xml',
  vue: 'xml',
  xml: 'xml',
  yaml: 'yaml',
  yml: 'yaml',
}

/// Map a file path to a registered hljs language, or null when the extension
/// is unknown (the line then renders as escaped plain text).
export function langForPath(path: string | null | undefined): string | null {
  if (!path) return null
  const base = path.slice(path.lastIndexOf('/') + 1)
  const dot = base.lastIndexOf('.')
  if (dot <= 0) return null
  const ext = base.slice(dot + 1).toLowerCase()
  // hasOwn guard: a file named e.g. `x.constructor` would otherwise hit
  // Object.prototype and return a function instead of a language name.
  return Object.hasOwn(EXT_TO_LANG, ext) ? EXT_TO_LANG[ext] : null
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;')
}

/// Highlight one diff line. Always returns HTML-safe markup: hljs escapes
/// the input, and the no-language / error paths escape it explicitly.
// Lines longer than this render as plain text: grammar regexes over
// megabyte-scale single lines (minified bundles, data blobs) cost hundreds
// of milliseconds and tens of MB, and such lines aren't hand-read anyway.
const MAX_HIGHLIGHT_LINE_LENGTH = 2000

export function highlightLine(content: string, lang: string | null): string {
  // getLanguage guard keeps hljs from logging a console warning for
  // unregistered languages; the catch covers grammar failures.
  if (lang && content.length <= MAX_HIGHLIGHT_LINE_LENGTH && hljs.getLanguage(lang)) {
    try {
      return hljs.highlight(content, { language: lang, ignoreIllegals: true }).value
    } catch {
      // Grammar failure — fall through to plain.
    }
  }
  return escapeHtml(content)
}
