<script lang="ts">
	import * as api from '$lib/api';
	import LazyGroup from './LazyGroup.svelte';
	import Lightbox from './Lightbox.svelte';
	import { bytes, count, date, fileName, parentName, similarity } from '$lib/format';
	import { autoLoad } from '$lib/scroll';
	import { store } from '$lib/store.svelte';
	import type { KeepRule, SimilarGroup, SimilarImage } from '$lib/types';

	let rule = $state<KeepRule>('highestResolution');
	/** The cluster being viewed full size, and which of its photos. */
	let preview = $state<{ images: SimilarImage[]; index: number } | null>(null);

	function paths(group: SimilarGroup): string[] {
		return group.images.map((i) => i.file.path);
	}

	/**
	 * Shape of the frames in one cluster, taken from its reference image.
	 *
	 * A fixed 4:3 frame put huge grey bars either side of every portrait photo.
	 * Photos in a cluster are near-copies of each other, so they share an
	 * orientation, and matching the frame to them removes the bars entirely.
	 * One ratio per cluster rather than one per photo, so rows stay even.
	 */
	function frameRatio(group: SimilarGroup): number {
		const reference = group.images[0];
		if (!reference || reference.height === 0) return 4 / 3;

		// Clamp so a panorama does not collapse the frame to a letterbox slit,
		// and a very tall crop does not make one tile the height of the page.
		return Math.min(1.9, Math.max(0.62, reference.width / reference.height));
	}

	/**
	 * How many "keep this one" buttons are worth offering.
	 *
	 * Past a handful they stop being a shortcut and start being a wall; the
	 * toolbar's keep-rule does the same job for a large group in one click.
	 */
	const KEEP_LIMIT = 8;

	/**
	 * Whether naming the folders would actually tell the copies apart.
	 *
	 * When every copy sits in the same folder the buttons would all carry the
	 * same label, so the row is noise and the checkboxes are the real tool.
	 */
	function foldersDiffer(paths: string[]): boolean {
		const first = parentName(paths[0] ?? '');
		return paths.some((path) => parentName(path) !== first);
	}

	const remaining = $derived(store.similarTotal - store.similar.length);
</script>

{#if preview}
	<Lightbox
		images={preview.images}
		bind:index={preview.index}
		onclose={() => (preview = null)}
	/>
{/if}

{#if store.similarTotal === 0}
	<div class="empty">
		{#if store.activity === 'scanning'}
			<h2>Scanning…</h2>
			<p>Look-alike photos appear here as soon as the scan finishes.</p>
		{:else if !store.scanConfig.detectors.similarImages}
			<h2>No look-alike photos found</h2>
			<p>Photo comparison was switched off for this scan.</p>
		{:else}
			<h2>No look-alike photos found</h2>
			<p>Nothing in the folders you scanned resembles anything else closely enough.</p>
		{/if}
	</div>
{:else}
	<div class="toolbar">
		<span class="count">
			<strong>{count(store.similarTotal)}</strong> clusters ·
			<strong>{count(store.summary?.similarImages ?? 0)}</strong> photos
		</span>

		<span class="spacer"></span>

		<label class="rule">
			Keep the
			<select bind:value={rule}>
				<option value="highestResolution">largest version</option>
				<option value="oldest">original (oldest capture date)</option>
				<option value="newest">newest</option>
				<option value="shortestPath">one with the shortest path</option>
			</select>
		</label>
		<button onclick={() => store.autoSelectSimilar(rule)}>Select the rest</button>
		<button onclick={() => store.clearSelection()} disabled={store.selectedCount === 0}>
			Clear
		</button>
	</div>

	<div class="list">
		{#each store.similar as group, index (group.id)}
			{@const siblings = paths(group)}
			<!-- Only the groups that can already be on screen render up front;
			     the observer brings in the rest. -->
			<LazyGroup estimate={340} eager={index < 4}>
				<article class="group">
					<header>
						<strong>{group.images.length} similar photos</strong>
						<span class="meta">
							{bytes(group.images.reduce((n, i) => n + i.file.size, 0))} total
						</span>
					</header>

					<!-- The tile count bounds the grid width, so a small cluster does not
					     get a full-width row of reserved, empty tracks. -->
					<div class="grid" style:--tiles={group.images.length} style:--frame={frameRatio(group)}>
						{#each group.images as image (image.file.path)}
							<!-- The tools sit beside the label, not inside it: buttons nested
							     in a <label> are invalid, get folded into the checkbox's
							     accessible name, and need preventDefault to stop the click
							     toggling selection. -->
							<div class="tile" class:marked={store.isSelected(image.file.path)}>
								<label class="body">
									<input
										type="checkbox"
										checked={store.isSelected(image.file.path)}
										onchange={() => store.toggle(image.file.path, siblings)}
									/>

									<img
										src={api.thumbnailUrl(image.file.path, 224)}
										alt={fileName(image.file.path)}
										loading="lazy"
										decoding="async"
									/>

									<div class="info">
										<span class="name" title={image.file.path}>{fileName(image.file.path)}</span>
										<span class="dims">
											{image.width}×{image.height} · {bytes(image.file.size)}
										</span>
										<span class="sub">
											<!-- Several photos in a cluster can be an exact perceptual
											     match, so "reference" would be true of more than one.
											     Say what is actually known instead. -->
											{#if image.distance === 0}
												identical
											{:else}
												{similarity(image.distance)} match
											{/if}
											· {date(image.takenAt ?? image.file.mtime)}
											{#if image.takenAt}<span class="tag">taken</span>{/if}
										</span>
									</div>
								</label>

								<!-- Two separate affordances, because one button labelled
								     "Show" next to a photo promises a bigger picture and
								     then opens a file manager instead. -->
								<span class="tools">
									<button
										title="View larger"
										aria-label="View larger"
										onclick={() =>
											(preview = {
												images: group.images,
												index: group.images.indexOf(image)
											})}
									>
										<svg viewBox="0 0 16 16" aria-hidden="true">
											<path
												d="M6 2H2v4M10 2h4v4M6 14H2v-4M10 14h4v-4"
												fill="none"
												stroke="currentColor"
												stroke-width="1.6"
												stroke-linecap="round"
											/>
										</svg>
									</button>

									<button
										title="Show in Explorer"
										aria-label="Show in Explorer"
										onclick={() => api.revealInExplorer(image.file.path)}
									>
										<svg viewBox="0 0 16 16" aria-hidden="true">
											<path
												d="M1.6 12.4V3.6h4l1.2 1.6h7.6v7.2z"
												fill="none"
												stroke="currentColor"
												stroke-width="1.4"
												stroke-linejoin="round"
											/>
										</svg>
									</button>
								</span>
							</div>
						{/each}
					</div>

					{#if siblings.length <= KEEP_LIMIT && foldersDiffer(siblings)}
						<footer>
							<span class="footer-label">Keep the copy in</span>
							{#each group.images as image (image.file.path)}
								<!-- Copies in a cluster usually share a resolution, so labelling
								     the buttons with it told the user nothing. The folder is what
								     distinguishes them. -->
								<button
									class="keep"
									title={image.file.path}
									onclick={() => store.selectAllBut(image.file.path, siblings)}
								>
									{parentName(image.file.path)}
								</button>
							{/each}
					</footer>
					{/if}
				</article>
			</LazyGroup>
		{/each}
	</div>

	{#if remaining > 0}
		<!-- A sentinel rather than a button: the next page starts loading a
	     screen before the user reaches the end, so the list never stops. -->
	<div class="more" use:autoLoad={() => store.loadMoreSimilar()}>
		{#if store.loadingMore}
			<span class="loading">Loading more clusters…</span>
		{:else}
			<button onclick={() => store.loadMoreSimilar()}>
				Load the remaining {count(store.similarTotal - store.similar.length)} clusters
			</button>
		{/if}
	</div>
	{/if}
{/if}

<style>
	.toolbar {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 12px 20px;
		border-bottom: 1px solid var(--border);
		background: var(--surface);
		position: sticky;
		top: 0;
		z-index: 2;
	}

	.spacer {
		flex: 1;
	}

	.count,
	.rule {
		color: var(--text-dim);
	}

	.rule {
		display: flex;
		align-items: center;
		gap: 6px;
	}

	.list {
		padding: 16px 20px;
		display: grid;
		/*
		 * `minmax(0, 1fr)` rather than the default `auto`: a grid column sized
		 * `auto` grows to its content's max-content width, and a wide group
		 * would then drag the whole list past the container and give the page a
		 * horizontal scrollbar.
		 */
		grid-template-columns: minmax(0, 1fr);
		gap: 12px;
	}

	.group {
		background: var(--surface);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		overflow: hidden;
	}

	header {
		display: flex;
		align-items: baseline;
		gap: 10px;
		padding: 9px 12px;
		background: var(--surface-2);
		border-bottom: 1px solid var(--border);
	}

	.meta {
		color: var(--text-dim);
		font-size: 12.5px;
	}

	.grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(184px, 1fr));
		gap: 10px;
		padding: 12px;
		/*
		 * `auto-fill` makes as many tracks as the container can hold, whether
		 * or not there are photos to put in them — so widening the window gave
		 * a three-photo cluster a row of empty columns. Capping the grid at the
		 * width its own tiles need keeps rows even (unlike `auto-fit`, which
		 * would stretch two photos into two posters) while leaving large
		 * clusters free to use the whole width.
		 */
		max-width: calc(var(--tiles, 12) * 244px);
	}

	.tile {
		position: relative;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		overflow: hidden;
		background: var(--surface);
	}

	.body {
		display: flex;
		flex-direction: column;
		cursor: pointer;
	}

	.tile:hover {
		border-color: var(--text-faint);
	}

	.tile.marked {
		border-color: var(--danger);
		background: var(--danger-soft);
	}

	.body input {
		position: absolute;
		top: 8px;
		left: 8px;
		z-index: 1;
		/* The thumbnail behind it can be any colour. */
		box-shadow: 0 0 0 2px rgb(0 0 0 / 0.35);
		border-radius: 3px;
	}

	.tile img {
		display: block;
		width: 100%;
		/*
		 * A fixed pixel height meant the frame's shape changed as the tile grew
		 * with the window, so the bars beside a photo got wider the more the
		 * window was widened. A ratio keeps the frame constant, and what is
		 * left over is only ever the photo's own shape.
		 */
		aspect-ratio: var(--frame, 1.3333);
		height: auto;
		/* `contain`, not `cover`: deciding whether two photos are the same copy
		   means seeing the whole frame, not a crop of it. `scale-down` on top
		   of that so a small image is not upscaled into a blur. */
		object-fit: scale-down;
		background: var(--photo-mat);
	}

	.info {
		display: grid;
		gap: 1px;
		padding: 7px 9px;
		min-width: 0;
	}

	.name {
		font-weight: 500;
		font-size: 12.5px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.dims {
		font-size: 12px;
		color: var(--text-dim);
		font-variant-numeric: tabular-nums;
	}

	.sub {
		font-size: 11.5px;
		color: var(--text-faint);
	}

	.tag {
		display: inline-block;
		margin-left: 4px;
		padding: 0 5px;
		border-radius: 999px;
		background: var(--surface-2);
		font-size: 10px;
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.tools {
		position: absolute;
		top: 6px;
		right: 6px;
		display: flex;
		gap: 4px;
		opacity: 0;
		transition: opacity 0.12s ease;
	}

	.tile:hover .tools,
	.tools:focus-within {
		opacity: 1;
	}

	.tools button {
		display: grid;
		place-items: center;
		width: 26px;
		height: 26px;
		padding: 0;
		/* The thumbnail behind can be any colour, so the buttons carry their
		   own ground rather than relying on contrast with the photo. */
		background: var(--surface);
		box-shadow: 0 1px 3px rgb(0 0 0 / 0.3);
	}

	.tools svg {
		width: 15px;
		height: 15px;
	}

	footer {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px;
		padding: 0 12px 12px;
	}

	.footer-label {
		color: var(--text-faint);
		font-size: 12px;
	}

	.keep {
		padding: 3px 9px;
		font-size: 12px;
	}

	.more {
		display: flex;
		justify-content: center;
		min-height: 44px;
		align-items: center;
		padding: 0 20px 24px;
	}

	.loading {
		color: var(--text-faint);
		font-size: 12.5px;
	}

	.empty {
		padding: 60px 20px;
		text-align: center;
		color: var(--text-dim);
	}

	.empty h2 {
		color: var(--text);
		font-size: 16px;
		margin: 0 0 6px;
	}
</style>
