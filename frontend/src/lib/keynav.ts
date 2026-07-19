import type { Verdict } from './types'

export type KeyAction =
  | { type: 'move'; delta: 1 | -1 }
  | { type: 'verdict'; verdict: Verdict }
  | { type: 'focus-comment' }
  | { type: 'confirm-submit' }
  | null

const EDITABLE_TAGS = new Set(['INPUT', 'TEXTAREA', 'SELECT'])

/**
 * Maps a keydown to an action. Returns null when typing in an
 * input/textarea/select or a contenteditable element, and null for any key
 * without a binding.
 */
export function interpretKey(
  key: string,
  targetTag: string,
  targetEditable: boolean,
): KeyAction {
  if (targetEditable || EDITABLE_TAGS.has(targetTag)) {
    return null
  }

  switch (key) {
    case 'j':
      return { type: 'move', delta: 1 }
    case 'k':
      return { type: 'move', delta: -1 }
    case 'a':
      return { type: 'verdict', verdict: 'approve' }
    case 'x':
      return { type: 'verdict', verdict: 'request-changes' }
    case 'c':
      return { type: 'verdict', verdict: 'comment' }
    case 'i':
      return { type: 'focus-comment' }
    case 'Enter':
      return { type: 'confirm-submit' }
    default:
      return null
  }
}
