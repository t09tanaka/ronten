// Tiny hand-rolled markdown converter for concern descriptions: paragraphs,
// `- ` bullet lists, `code` spans, and fenced ``` code blocks. No external
// markdown dependency. HTML is escaped first since descriptions come from
// agent-supplied JSON.
//
// Trojan Source / control-character defense (`revealInvisibles`, see
// `invisibles.ts`) is applied to each line individually, AFTER splitting the
// source on '\n' — never to the raw multi-line source beforehand. Applying
// it first would tokenize every literal newline into `⟨U+000A⟩` before this
// parser ever saw a line break, collapsing every paragraph, bullet list, and
// code fence into one flat, garbled block. Splitting first means each line
// handed to `revealInvisibles` contains no LF to tokenize, so ESC/C1/DEL/
// bidi characters still get revealed per line while paragraph/list/fence
// structure survives intact. TAB stays literal, per `revealInvisibles`'s
// line-content policy (matches how diff line content is rendered).

import { revealInvisibles } from './invisibles'

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;')
}

function renderInline(s: string): string {
  return s.replace(/`([^`]+)`/g, '<code>$1</code>')
}

export function renderMarkdown(src: string): string {
  const lines = escapeHtml(src)
    .split('\n')
    .map((line) => revealInvisibles(line))
  const out: string[] = []
  let paragraph: string[] = []
  let list: string[] = []

  function flushParagraph(): void {
    if (paragraph.length > 0) {
      out.push(`<p>${renderInline(paragraph.join(' '))}</p>`)
      paragraph = []
    }
  }
  function flushList(): void {
    if (list.length > 0) {
      out.push(`<ul>${list.map((item) => `<li>${renderInline(item)}</li>`).join('')}</ul>`)
      list = []
    }
  }

  let i = 0
  while (i < lines.length) {
    const line = lines[i]
    if (line.startsWith('```')) {
      flushParagraph()
      flushList()
      const codeLines: string[] = []
      i++
      while (i < lines.length && !lines[i].startsWith('```')) {
        codeLines.push(lines[i])
        i++
      }
      out.push(`<pre><code>${codeLines.join('\n')}</code></pre>`)
      i++ // skip closing fence
      continue
    }
    if (line.startsWith('- ')) {
      flushParagraph()
      list.push(line.slice(2))
      i++
      continue
    }
    if (line.trim() === '') {
      flushParagraph()
      flushList()
      i++
      continue
    }
    flushList()
    paragraph.push(line.trim())
    i++
  }
  flushParagraph()
  flushList()
  return out.join('')
}
