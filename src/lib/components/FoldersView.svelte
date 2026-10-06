<script lang="ts">
	/**
	 * The same duplicates as the file list, grouped the way the decision is
	 * actually made: "I have this folder six times, keep one."
	 *
	 * Everything here resolves to per-file plans, so the action bar and every
	 * safety rule behind it are untouched.
	 */

	import * as api from '$lib/api';
	import LazyGroup from './LazyGroup.svelte';
	import { bytes, count, folderName, shortenPath } from '$lib/format';
	import { autoLoad } from '$lib/scroll';
	import { store } from '$lib/store.svelte';
	import type { KeepRule } from '$lib/types';

	let rule = $state<KeepRule>('shortestPath');

	const remaining = $derived(store.folderTotal - store.folders.length);
</script>

{#if store.folderTotal === 0}
	<div class="empty">
		{#if store.activity === 'scanning'}
			<h2>Scanning…</h2>
			<p>Duplicated folders appear here as soon as the scan finishes.</p>
		{:else}
			<h2>No duplicated folders found</h2>
			<p>
				No two folders hold the same set of files. Individual duplicates may still exist — see the
				Duplicates tab.
			</p>
		{/if}
	</div>
{:else}
	<div class="toolbar">
		<span class="count">
			<strong>{count(store.folderTotal)}</strong> duplicated folders ·
			<strong>{bytes(store.folderReclaimable)}</strong> reclaimable
		</span>

		<span class="spacer"></span>

		<label class="rule">
			Keep the
			<select bind:value={rule}>
				<option value="shortestPath">one with the shortest path</option>
				<option value="oldest">shallowest one</option>
			</select>
		</label>
		<button onclick={() => store.autoSelectFolders(rule)}>Select the rest</button>
		<button onclick={() => store.clearSelection()} disabled={store.selectedCount === 0}>
			Clear
		</button>
	</div>

	<div class="list">
		{#each store.folders as group, index (group.id)}
			<LazyGroup estimate={200} eager={index < 6}>
				<article class="group">
					<header>
						<strong>{group.folders.length} copies of a folder</strong>
						<span class="meta">
							{count(group.sharedFiles)} shared files · {bytes(group.sharedBytes)} each ·
							{bytes(group.sharedBytes * (group.folders.length - 1))} reclaimable
						</span>
						{#if group.interchangeable}
							<span class="badge ok">Identical</span>
						{:else}
							<!-- Said plainly: one of these holds something the others do
							     not, so they are not interchangeable. -->
							<span class="badge warn">Some hold unique files</span>
						{/if}
					</header>

					{#each group.folders as folder (folder.path)}
						<div class="folder">
							<span class="path" title={folder.path}>{shortenPath(folder.path, 90)}</span>
							<span class="stat">{count(folder.totalFiles)} files</span>
							<span class="stat" class:unique={folder.uniqueFiles > 0}>
								{folder.uniqueFiles > 0 ? `${count(folder.uniqueFiles)} unique` : '—'}
							</span>
							<button
								class="reveal"
								title="Show this folder in Explorer"
								onclick={() => api.revealInExplorer(folder.path)}
							>
								Open folder
							</button>
						</div>
					{/each}

					<footer>
						<span class="footer-label">Keep the copy in</span>
						{#each group.folders as folder (folder.path)}
							<button
								class="keep"
								title={folder.path}
								onclick={() => store.keepFolder(group.id, folder.path)}
							>
								{folderName(folder.path)}
							</button>
						{/each}
					</footer>
				</article>
			</LazyGroup>
		{/each}
	</div>

	{#if remaining > 0}
		<div class="more" use:autoLoad={() => store.loadMoreFolders()}>
			{#if store.loadingMore}
				<span class="loading">Loading more folders…</span>
			{:else}
				<button onclick={() => store.loadMoreFolders()}>
					Load the remaining {count(remaining)} folders
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

	.badge {
		margin-left: auto;
		padding: 1px 8px;
		border-radius: 999px;
		font-size: 11px;
		font-weight: 600;
		white-space: nowrap;
	}

	.badge.ok {
		background: var(--ok-soft);
		color: var(--ok);
	}

	.badge.warn {
		background: var(--warn-soft);
		color: var(--warn);
	}

	.folder {
		display: grid;
		grid-template-columns: minmax(0, 1fr) 80px 90px auto;
		align-items: center;
		gap: 12px;
		padding: 7px 12px;
		border-bottom: 1px solid var(--border);
	}

	.path {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: 12.5px;
	}

	.stat {
		font-size: 12px;
		color: var(--text-dim);
		text-align: right;
		font-variant-numeric: tabular-nums;
	}

	.stat.unique {
		color: var(--warn);
		font-weight: 600;
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
		max-width: 280px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.more {
		display: flex;
		justify-content: center;
		align-items: center;
		min-height: 44px;
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
