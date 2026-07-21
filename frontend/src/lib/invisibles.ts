// Trojan Source (CVE-2021-42574) defenses: bidi control characters can
// reorder how code *displays* without changing its logical byte order, and
// zero-width characters can hide entirely. Since this tool exists so a human
// can approve agent-written diffs, any such character in diff content must be
// made visible rather than silently rendered. Call `revealInvisibles` on the
// plain line content *before* it enters the escape/highlight pipeline, so the
// visible ⟨U+XXXX⟩ markers flow through the existing HTML-escaping and stay
// XSS-safe.

/** Bidi control characters and zero-width/invisible characters to reveal. */
const INVISIBLE_CODEPOINTS: readonly number[] = [
  0x202a, 0x202b, 0x202c, 0x202d, 0x202e, // LRE RLE PDF LRO RLO
  0x2066, 0x2067, 0x2068, 0x2069, // LRI RLI FSI PDI
  0x200b, 0x200c, 0x200d, 0x2060, // ZWSP ZWNJ ZWJ WJ
  0xfeff, // ZWNBSP/BOM
  0x00ad, // SOFT HYPHEN
]

const INVISIBLE_SET = new Set(INVISIBLE_CODEPOINTS)

function toToken(codePoint: number): string {
  return `⟨U+${codePoint.toString(16).toUpperCase().padStart(4, '0')}⟩`
}

const INVISIBLE_PATTERN = new RegExp(
  `[${INVISIBLE_CODEPOINTS.map((cp) => `\\u{${cp.toString(16)}}`).join('')}]`,
  'gu',
)

/** Replace Trojan Source bidi controls and invisible characters with visible ⟨U+XXXX⟩ tokens. */
export function revealInvisibles(s: string): string {
  return s.replace(INVISIBLE_PATTERN, (ch) => toToken(ch.codePointAt(0) ?? 0))
}

/** True if `s` contains any bidi control or invisible character from the list. */
export function hasInvisibles(s: string): boolean {
  for (const ch of s) {
    if (INVISIBLE_SET.has(ch.codePointAt(0) ?? -1)) return true
  }
  return false
}
