import type { Verdict } from './types'

export type KeyAction =
  | { type: 'move'; delta: 1 | -1 }
  | { type: 'verdict'; verdict: Verdict }
  | { type: 'focus-comment' }
  | { type: 'confirm-submit' }
  | null

export interface KeyInput {
  key: string
  ctrlKey: boolean
  metaKey: boolean
  altKey: boolean
  isComposing: boolean
  targetTag: string
  targetEditable: boolean
}

const EDITABLE_TAGS = new Set(['INPUT', 'TEXTAREA', 'SELECT'])

/**
 * Maps a keydown to an action. Returns null when a modifier is held or an
 * IME composition is in progress (Cmd+C must stay copy, not become a
 * verdict), when typing in an input/textarea/select or a contenteditable
 * element, and for any key without a binding.
 */
export function interpretKey(input: KeyInput): KeyAction {
  if (input.ctrlKey || input.metaKey || input.altKey || input.isComposing) {
    return null
  }
  if (input.targetEditable || EDITABLE_TAGS.has(input.targetTag)) {
    return null
  }

  switch (input.key) {
    case 'j':
      return { type: 'move', delta: 1 }
    case 'k':
      return { type: 'move', delta: -1 }
    case 'a':
      return { type: 'verdict', verdict: 'approve' }
    case 'x':
      return { type: 'verdict', verdict: 'request-changes' }
    case 'i':
      return { type: 'focus-comment' }
    case 'Enter':
      return { type: 'confirm-submit' }
    default:
      return null
  }
}
