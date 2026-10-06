<script lang="ts">
	/**
	 * Full-size look at one photo in a cluster, with the rest a keystroke away.
	 *
	 * Deciding which near-copy to keep is a comparison, not an inspection: the
	 * useful question is "how does this one differ from the next one", and at
	 * 224 pixels that is unanswerable. So the arrows step through the cluster
	 * without leaving the overlay, and the caption carries the numbers the
	 * decision actually turns on — resolution, size, capture date, folder.
	 */

	import * as api from '$lib/api';
	import { bytes, date, fileName, similarity } from '$lib/format';
	import type { SimilarImage } from '$lib/types';

	interface Props {
		images: SimilarImage[];
		index: number;
		onclose: () => void;
	}

	let { images, index = $bindable(), onclose }: Props = $props();

	let panel = $state<HTMLElement>();

	const current = $derived(images[index]);

	// Move focus into the overlay so the arrow keys and Escape work without the
	// user having to click something first.
	$effect(() => {
		panel?.focus();
	});

	function step(delta: number) {
		index = (index + delta + images.length) % images.length;
	}

	function onkeydown(event: KeyboardEvent) {
		switch (event.key) {
			case 'Escape':
				onclose();
				break;
			case 'ArrowLeft':
				step(-1);
				break;
			case 'ArrowRight':
				step(1);
				break;
		}
	}
</script>

<svelte:window {onkeydown} />

<div class="backdrop" role="dialog" aria-modal="true" aria-label={fileName(current.file.path)}>
	<!-- Clicking outside closes. A real button rather than a click handler on
	     the backdrop div, so it is reachable and announced like any other
	     control instead of being a trap for anyone not using a mouse. -->
	<button class="dismiss" onclick={onclose} aria-label="Close the preview"></button>

	<div class="panel" bind:this={panel} tabindex="-1">
		<!-- The image needs a wrapper that owns the height. A percentage
		     max-height on an image inside a `1fr` row is circular, so browsers
		     treat it as `auto` and the photo pushes the caption off screen. -->
		<div class="frame">
			<img
				src={api.thumbnailUrl(current.file.path, 1024)}
				alt={fileName(current.file.path)}
				decoding="async"
			/>
		</div>

		<div class="caption">
			<div class="line">
				<strong>{fileName(current.file.path)}</strong>
				<span class="dim">{current.width}×{current.height} · {bytes(current.file.size)}</span>
				<span class="dim">
					{current.distance === 0 ? 'identical' : `${similarity(current.distance)} match`}
					· {date(current.takenAt ?? current.file.mtime)}
				</span>
			</div>
			<code class="path" title={current.file.path}>{current.file.path}</code>
		</div>

		<div class="controls">
			<button onclick={() => step(-1)} disabled={images.length < 2} title="Previous (←)">‹</button>
			<span class="position">{index + 1} of {images.length}</span>
			<button onclick={() => step(1)} disabled={images.length < 2} title="Next (→)">›</button>

			<span class="spacer"></span>

			<button onclick={() => api.revealInExplorer(current.file.path)}>Open folder</button>
			<button onclick={onclose} title="Close (Esc)">Close</button>
		</div>
	</div>
</div>

<style>
	.backdrop {
		position: fixed;
		inset: 0;
		z-index: 50;
		display: grid;
		place-items: center;
		padding: 24px;
		background: rgb(0 0 0 / 0.62);
	}

	.dismiss {
		position: absolute;
		inset: 0;
		border: none;
		background: none;
		padding: 0;
		cursor: default;
	}

	.panel {
		position: relative;
		display: grid;
		grid-template-rows: minmax(0, 1fr) auto auto;
		gap: 10px;
		max-width: min(1100px, 100%);
		max-height: calc(100vh - 48px);
		padding: 12px;
		background: var(--surface);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		box-shadow: 0 12px 40px rgb(0 0 0 / 0.35);
	}

	.panel:focus {
		outline: none;
	}

	.frame {
		display: grid;
		min-height: 0;
		overflow: hidden;
		background: var(--photo-mat);
		border-radius: 4px;
	}

	img {
		display: block;
		width: 100%;
		height: 100%;
		min-height: 0;
		/* `scale-down`, not `contain`: a 256-pixel image should be shown at 256
		   pixels, not stretched across the panel and made blurry. */
		object-fit: scale-down;
	}

	.caption {
		display: grid;
		gap: 3px;
		min-width: 0;
	}

	.line {
		display: flex;
		flex-wrap: wrap;
		align-items: baseline;
		gap: 10px;
	}

	.dim {
		color: var(--text-dim);
		font-size: 12.5px;
		font-variant-numeric: tabular-nums;
	}

	.path {
		color: var(--text-faint);
		font-size: 12px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.controls {
		display: flex;
		align-items: center;
		gap: 8px;
	}

	.controls button:first-child,
	.controls button:nth-child(3) {
		font-size: 18px;
		line-height: 1;
		padding: 2px 12px;
	}

	.position {
		color: var(--text-dim);
		font-size: 12.5px;
		font-variant-numeric: tabular-nums;
	}

	.spacer {
		flex: 1;
	}
</style>
