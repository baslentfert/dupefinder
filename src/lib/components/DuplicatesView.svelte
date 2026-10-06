<script lang="ts">
	import * as api from '$lib/api';
	import LazyGroup from './LazyGroup.svelte';
	import { bytes, count, date, directory, fileName, parentName } from '$lib/format';
	import { autoLoad } from '$lib/scroll';
	import { store } from '$lib/store.svelte';
	import type { DuplicateGroup, KeepRule } from '$lib/types';

	let rule = $state<KeepRule>('oldest');

	function paths(group: DuplicateGroup): string[] {
		return group.files.map((f) => f.path);
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

	const shown = $derived(store.duplicates.length);
	const remaining = $derived(store.duplicateTotal - shown);
</script>

{#if store.duplicateTotal === 0}
	<div class="empty">
		{#if store.activity === 'scanning'}
			<h2>Scanning…</h2>
			<p>Duplicates appear here as soon as the scan finishes.</p>
		{:else}
			<h2>No identical files found</h2>
			<p>Nothing in the folders you scanned appears more than once.</p>
		{/if}
	</div>
{:else}
	<div class="toolbar">
		<span class="count">
			<strong>{count(store.duplicateTotal)}</strong> groups
			{#if store.summary}
				· <strong>{bytes(store.summary.wastedBytes)}</strong> reclaimable
			{/if}
		</span>

		<span class="spacer"></span>

		<label class="rule">
			Keep the
			<select bind:value={rule}>
				<option value="oldest">oldest copy</option>
				<option value="newest">newest copy</option>
				<option value="shortestPath">one with the shortest path</option>
			</select>
		</label>
		<button onclick={() => store.autoSelectDuplicates(rule)}>Select the rest</button>
		<button onclick={() => store.clearSelection()} disabled={store.selectedCount === 0}>
			Clear
		</button>
	</div>

	<div class="list">
		{#each store.duplicates as group, index (group.id)}
			{@const siblings = paths(group)}
			<!-- Only the groups that can already be on screen render up front;
			     the observer brings in the rest. -->
			<LazyGroup estimate={180} eager={index < 8}>
				<article class="group">
					<header>
						<span class="size">{bytes(group.size)} each</span>
						<span class="meta">
							{group.files.length} copies · {bytes(group.size * (group.files.length - 1))} wasted
						</span>
						<code class="hash" title="BLAKE3 {group.hash}">{group.hash.slice(0, 12)}</code>
					</header>

					{#each group.files as file (file.path)}
						<label class="file" class:marked={store.isSelected(file.path)}>
							<input
								type="checkbox"
								checked={store.isSelected(file.path)}
								onchange={() => store.toggle(file.path, siblings)}
							/>
							<span class="name" title={file.path}>{fileName(file.path)}</span>
							<span class="dir" title={file.path}>{directory(file.path)}</span>
							<span class="date">{date(file.mtime)}</span>
							<button
								class="reveal"
								title="Show this file in Explorer"
								onclick={(e) => {
									e.preventDefault();
									api.revealInExplorer(file.path);
								}}
							>
								Open folder
							</button>
						</label>
					{/each}

					{#if siblings.length <= KEEP_LIMIT && foldersDiffer(siblings)}
						<footer>
							<span class="footer-label">Keep the copy in</span>
							{#each group.files as file (file.path)}
								<!-- The file names in a group are identical by definition, so the
								     folder is the only thing that tells the copies apart. -->
								<button
									class="keep"
									title={file.path}
									onclick={() => store.selectAllBut(file.path, siblings)}
								>
									{parentName(file.path)}
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
	<div class="more" use:autoLoad={() => store.loadMoreDuplicates()}>
		{#if store.loadingMore}
			<span class="loading">Loading more groups…</span>
		{:else}
			<button onclick={() => store.loadMoreDuplicates()}>
				Load the remaining {count(store.duplicateTotal - store.duplicates.length)} groups
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

	.count {
		color: var(--text-dim);
	}

	.rule {
		color: var(--text-dim);
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

	.size {
		font-weight: 600;
	}

	.meta {
		color: var(--text-dim);
		font-size: 12.5px;
	}

	.hash {
		margin-left: auto;
		color: var(--text-faint);
		font-size: 11.5px;
	}

	.file {
		display: grid;
		grid-template-columns: auto minmax(120px, 1fr) minmax(0, 2fr) 90px auto;
		align-items: center;
		gap: 10px;
		padding: 7px 12px;
		border-bottom: 1px solid var(--border);
		cursor: pointer;
	}

	.file:hover {
		background: var(--surface-2);
	}

	.file.marked {
		background: var(--danger-soft);
	}

	.name {
		font-weight: 500;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.dir {
		color: var(--text-dim);
		font-size: 12.5px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		direction: rtl;
		text-align: left;
	}

	.date {
		color: var(--text-faint);
		font-size: 12px;
		text-align: right;
		font-variant-numeric: tabular-nums;
	}

	.reveal {
		padding: 2px 8px;
		font-size: 12px;
		white-space: nowrap;
	}

	footer {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px;
		padding: 8px 12px;
	}

	.footer-label {
		color: var(--text-faint);
		font-size: 12px;
	}

	.keep {
		padding: 3px 9px;
		font-size: 12px;
		max-width: 260px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
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
