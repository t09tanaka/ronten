import { describe, expect, it } from 'vitest'
import { renderMarkdown } from './markdown'

describe('renderMarkdown', () => {
  it('renders paragraphs, bullet lists, and fenced code blocks as separate structural blocks', () => {
    const src = [
      'First paragraph.',
      '',
      '- item one',
      '- item two',
      '',
      '```',
      'code line one',
      'code line two',
      '```',
      '',
      'Second paragraph.',
    ].join('\n')

    const html = renderMarkdown(src)

    expect(html).toContain('<p>First paragraph.</p>')
    expect(html).toContain('<p>Second paragraph.</p>')
    expect(html).toContain('<ul><li>item one</li><li>item two</li></ul>')
    expect(html).toContain('<pre><code>code line one\ncode line two</code></pre>')
  })

  // Regression: revealInvisibles used to be applied to the whole multi-line
  // source *before* renderMarkdown split it on '\n', tokenizing every
  // literal newline into `⟨U+000A⟩` and destroying the line breaks this
  // parser relies on to find paragraph/list/fence boundaries — collapsing
  // everything into one flat, garbled paragraph. It must now be applied per
  // line, after the split, so structure survives while control characters
  // (ESC here) still get revealed.
  it('reveals control characters per line without destroying markdown structure', () => {
    const src = [
      'First paragraph with an esc\x1bape sequence.',
      '',
      '- item one',
      '- item two',
      '',
      '```',
      'code line one',
      'code line two',
      '```',
      '',
      'Second paragraph.',
    ].join('\n')

    const html = renderMarkdown(src)

    // (a) structure survives: still multiple distinct blocks, not one
    // flattened paragraph.
    expect(html).toContain('<p>First paragraph with an esc⟨U+001B⟩ape sequence.</p>')
    expect(html).toContain('<p>Second paragraph.</p>')
    expect(html).toContain('<ul><li>item one</li><li>item two</li></ul>')
    expect(html).toContain('<pre><code>code line one\ncode line two</code></pre>')

    // (b) the ESC character is revealed as a visible token.
    expect(html).toContain('⟨U+001B⟩')

    // (c) no raw ESC byte reaches the rendered output.
    expect(html.includes('\x1b')).toBe(false)
  })

  it('escapes HTML in descriptions and still renders inline code spans', () => {
    const html = renderMarkdown('a <script> tag and `inline code`')
    expect(html).toBe('<p>a &lt;script&gt; tag and <code>inline code</code></p>')
  })
})
