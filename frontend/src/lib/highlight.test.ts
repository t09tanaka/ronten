import { describe, expect, it } from 'vitest'
import { highlightLine, langForPath } from './highlight'

describe('langForPath', () => {
  it('maps known extensions to registered languages', () => {
    expect(langForPath('src/middleware/auth.ts')).toBe('typescript')
    expect(langForPath('src/main.rs')).toBe('rust')
    expect(langForPath('frontend/src/App.svelte')).toBe('xml')
    expect(langForPath('Cargo.toml')).toBe('ini')
    expect(langForPath('README.md')).toBe('markdown')
  })

  it('is case-insensitive on the extension', () => {
    expect(langForPath('LEGACY.SQL')).toBe('sql')
  })

  it('returns null for unknown or missing extensions', () => {
    expect(langForPath('bin/data.bin')).toBeNull()
    expect(langForPath('Makefile')).toBeNull()
    expect(langForPath(null)).toBeNull()
    expect(langForPath(undefined)).toBeNull()
  })

  it('ignores dot-directories and dotfiles without a real extension', () => {
    expect(langForPath('.gitignore')).toBeNull()
    expect(langForPath('.config/app.yaml')).toBe('yaml')
  })
})

describe('highlightLine', () => {
  it('wraps recognized tokens in hljs spans', () => {
    const out = highlightLine('const x = 1', 'typescript')
    expect(out).toContain('hljs-keyword')
    expect(out).toContain('const')
  })

  it('escapes HTML in code for every path', () => {
    const evil = '<img src=x onerror=alert(1)> & "quotes"'
    expect(highlightLine(evil, null)).not.toContain('<img')
    expect(highlightLine(evil, 'typescript')).not.toContain('<img')
  })

  it('falls back to escaped plain text for unregistered languages', () => {
    const out = highlightLine('let a = <b>', 'no-such-lang')
    expect(out).toBe('let a = &lt;b&gt;')
  })
})
