/// Scroll the general-comments box into view — used when a "comment"
/// verdict is chosen so the reviewer lands where the comment gets written.
/// Instant scroll on purpose: animated scrolling stalls in throttled or
/// embedded browser contexts, and an instant jump also satisfies
/// prefers-reduced-motion without a media query.
export function scrollToGeneralComments(): void {
  // Deferred one frame so the scroll runs after the verdict change's
  // re-render settles the layout.
  requestAnimationFrame(() => {
    document
      .getElementById('general-comment-textarea')
      ?.scrollIntoView({ behavior: 'auto', block: 'center' })
  })
}
