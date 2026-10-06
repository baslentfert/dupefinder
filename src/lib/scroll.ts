/** Shared scrolling helpers. */

/**
 * Nearest ancestor that actually scrolls, or `null` for the viewport.
 *
 * `IntersectionObserver` measures against its root, and `rootMargin` is
 * relative to that root — so an observer that wants "one screen ahead of the
 * list" has to be given the element the list scrolls in, not the document,
 * which never scrolls in this app.
 */
export function scrollParent(node: HTMLElement): HTMLElement | null {
	let parent = node.parentElement;
	while (parent) {
		const overflow = getComputedStyle(parent).overflowY;
		if (overflow === 'auto' || overflow === 'scroll') return parent;
		parent = parent.parentElement;
	}
	return null;
}

/**
 * Svelte action that calls `load` when the node comes within a screen of the
 * viewport.
 *
 * Used on the sentinel at the end of a result list, so the next page is
 * already arriving by the time the user reaches the bottom instead of waiting
 * behind a button they have to notice and press.
 */
export function autoLoad(node: HTMLElement, load: () => void) {
	let current = load;

	const observer = new IntersectionObserver(
		(entries) => {
			if (entries.some((entry) => entry.isIntersecting)) current();
		},
		{
			root: scrollParent(node),
			// Start fetching a screen early; arriving at an empty bottom is the
			// thing that reads as slow.
			rootMargin: '600px 0px',
			threshold: 0
		}
	);

	observer.observe(node);

	return {
		update(next: () => void) {
			current = next;
		},
		destroy() {
			observer.disconnect();
		}
	};
}
