<script lang="ts">
	/**
	 * Renders its children only while they are near the viewport.
	 *
	 * A scan of a photo library produces thousands of groups; mounting them all
	 * means tens of thousands of DOM nodes and, worse, an `<img>` for every
	 * photo. `loading="lazy"` stops the *fetches* but not the layout and paint
	 * cost, and it does nothing for the duplicate list, which has no images at
	 * all.
	 *
	 * So each group observes itself. Within `margin` of the scroll container it
	 * renders normally; outside it, the content is dropped and replaced by a
	 * spacer of the height it last measured — which is what keeps the scrollbar
	 * from jumping as groups come and go. Because the height is remembered, the
	 * page geometry is identical whether a group is mounted or not.
	 *
	 * Selection lives in the store keyed by path, so unmounting a group never
	 * loses what the user ticked, and thumbnail responses carry a cache header
	 * so remounting does not re-decode anything.
	 */

	import { untrack, type Snippet } from 'svelte';

	import { scrollParent } from '$lib/scroll';

	interface Props {
		/**
		 * How far beyond the scroll container to keep content mounted, as a
		 * CSS `rootMargin`. The default keeps one viewport above and one below
		 * the visible one — three screens of live DOM.
		 */
		margin?: string;
		/** Height to assume before the group has ever been measured. */
		estimate?: number;
		/**
		 * Render immediately instead of waiting for the observer.
		 *
		 * Set this on the handful of groups that will be on screen at once. The
		 * observer only reports after layout, so without it the first frame of a
		 * freshly opened tab is empty placeholders.
		 */
		eager?: boolean;
		children: Snippet;
	}

	let { margin = '100% 0px', estimate = 320, eager = false, children }: Props = $props();

	let element = $state<HTMLElement>();

	// Starting mounted would defeat the point: opening a tab with 200 groups
	// would build every one of them — every tile, every <img> — and only then
	// let the observer tear the offscreen ones back down. That cost about seven
	// seconds of frozen window on every switch to the photo tab.
	// Seeded from `eager`, then owned by the observer: `untrack` says that the
	// initial value is all we want, rather than a binding that would fight the
	// observer for control.
	let mounted = $state(untrack(() => eager));
	/** Undefined until the group has been rendered and measured once. */
	let measured = $state<number>();

	const spacer = $derived(measured ?? estimate);

	$effect(() => {
		const node = element;
		if (!node) return;

		const observer = new IntersectionObserver(
			(entries) => {
				const entry = entries[entries.length - 1];
				if (!entry) return;

				if (entry.isIntersecting) {
					mounted = true;
					return;
				}

				// Measure before unmounting: afterwards the box is only the
				// spacer, and reading it would lock in the wrong height.
				const height = node.getBoundingClientRect().height;
				if (height > 0) measured = height;
				mounted = false;
			},
			{
				// `rootMargin` is relative to the root, so the root has to be the
				// element that scrolls — not the document, which never does here.
				root: scrollParent(node),
				rootMargin: margin,
				threshold: 0
			}
		);

		observer.observe(node);
		return () => observer.disconnect();
	});
</script>

<div bind:this={element} style:min-height={mounted ? undefined : `${spacer}px`}>
	{#if mounted}
		{@render children()}
	{/if}
</div>
