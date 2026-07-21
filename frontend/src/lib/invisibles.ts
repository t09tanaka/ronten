// Trojan Source (CVE-2021-42574) defenses: bidi control characters can
// reorder how code *displays* without changing its logical byte order, and
// zero-width characters can hide entirely. Since this tool exists so a human
// can approve agent-written diffs, any such character in diff content must be
// made visible rather than silently rendered. Call `revealInvisibles` on the
// plain line content *before* it enters the escape/highlight pipeline, so the
// visible ⟨U+XXXX⟩ markers flow through the existing HTML-escaping and stay
// XSS-safe.
//
// This also covers plain C0/C1 control characters, DEL, and the JS
// line/paragraph separators (U+2028/U+2029): a forged newline or ESC
// (ANSI/OSC sequence) inside a path or a single diff line can inject fake
// content or alter how the surrounding UI displays, the same threat as the
// bidi/zero-width characters above. Two variants exist because TAB (U+0009)
// is legitimate, common, and harmless inside diff line content (it renders
// as indentation) but not inside a path/title/identifier, where a raw tab
// has no legitimate reason to appear and can still be used to misalign
// rendered or copy-pasted text:
//   - `revealInvisibles`: line content — TAB stays literal.
//   - `revealControlChars`: paths/titles/identifiers — TAB is escaped too.

/** Bidi control characters and zero-width/invisible characters to reveal. */
const INVISIBLE_CODEPOINTS: readonly number[] = [
  0x202a, 0x202b, 0x202c, 0x202d, 0x202e, // LRE RLE PDF LRO RLO
  0x2066, 0x2067, 0x2068, 0x2069, // LRI RLI FSI PDI
  0x200e, 0x200f, 0x061c, // LRM RLM ALM
  0x200b, 0x200c, 0x200d, 0x2060, // ZWSP ZWNJ ZWJ WJ
  0xfeff, // ZWNBSP/BOM
  0x00ad, // SOFT HYPHEN
]

const INVISIBLE_SET = new Set(INVISIBLE_CODEPOINTS)

const TAB = 0x09

/** C0 control (U+0000–U+001F), DEL (U+007F), C1 control (U+0080–U+009F), or the JS line/paragraph separators (U+2028/U+2029). */
function isPlainControlChar(cp: number): boolean {
  return (
    (cp >= 0x00 && cp <= 0x1f) ||
    cp === 0x7f ||
    (cp >= 0x80 && cp <= 0x9f) ||
    cp === 0x2028 ||
    cp === 0x2029
  )
}

function needsEscape(cp: number, includeTab: boolean): boolean {
  if (INVISIBLE_SET.has(cp)) return true
  if (cp === TAB) return includeTab
  return isPlainControlChar(cp)
}

function toToken(codePoint: number): string {
  return `⟨U+${codePoint.toString(16).toUpperCase().padStart(4, '0')}⟩`
}

function revealWith(s: string, includeTab: boolean): string {
  let out = ''
  let changed = false
  for (const ch of s) {
    const cp = ch.codePointAt(0) ?? 0
    if (needsEscape(cp, includeTab)) {
      out += toToken(cp)
      changed = true
    } else {
      out += ch
    }
  }
  return changed ? out : s
}

/**
 * Replace Trojan Source bidi/invisible characters and control characters
 * with visible ⟨U+XXXX⟩ tokens, for diff line content. TAB is left literal
 * (it renders as ordinary indentation).
 */
export function revealInvisibles(s: string): string {
  return revealWith(s, false)
}

/**
 * Superset of `revealInvisibles` for paths, titles, and other short
 * identifiers: TAB is also escaped, since it has no legitimate reason to
 * appear in these fields and can still be used to misalign displayed text.
 */
export function revealControlChars(s: string): string {
  return revealWith(s, true)
}

function hasEscapable(s: string, includeTab: boolean): boolean {
  for (const ch of s) {
    if (needsEscape(ch.codePointAt(0) ?? -1, includeTab)) return true
  }
  return false
}

/**
 * True if `s` contains any character that `revealInvisibles` would escape
 * (TAB excluded) — use for diff line content, so the "contains invisible
 * characters" badge agrees with what actually gets a visible token there
 * (an ordinary tab-indented line must not trip this).
 */
export function hasInvisibles(s: string): boolean {
  return hasEscapable(s, false)
}

/**
 * True if `s` contains any character that `revealControlChars` would escape
 * (TAB included) — use for paths/titles/identifiers.
 */
export function hasControlChars(s: string): boolean {
  return hasEscapable(s, true)
}

/** Null-tolerant `revealControlChars`, for agent-supplied path fields that may be absent. */
export function reveal(s: string | null | undefined): string | null | undefined {
  return s == null ? s : revealControlChars(s)
}
