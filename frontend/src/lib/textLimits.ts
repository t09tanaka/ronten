// Character counting that matches the server's (see `MAX_COMMENT_CHARS` in
// src/session.rs): Rust counts Unicode scalar values via `chars().count()`,
// but `String.prototype.length` counts UTF-16 code units — an astral
// character like "😀" is 1 scalar but 2 UTF-16 units. Using `.length` for a
// limit the server enforces by scalar count under-allows astral-heavy text
// (the UI would block input the server would actually accept) and, worse,
// native `maxlength` truncation by UTF-16 units can split a surrogate pair
// in half. Everywhere the UI counts or truncates comment text against
// max_comment_chars, it must go through these instead.

/** Unicode scalar-value length of `s` — matches Rust's `chars().count()`. */
export function scalarLength(s: string): number {
  return [...s].length
}

/** Truncates `s` to at most `max` Unicode scalars (not UTF-16 units), so a
 * truncation can never split an astral character's surrogate pair. */
export function truncateToScalars(s: string, max: number): string {
  const chars = [...s]
  return chars.length <= max ? s : chars.slice(0, max).join('')
}
