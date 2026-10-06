<script lang="ts">
	import * as api from '$lib/api';
	import { bytes, confidenceLabel, count, date, orphanKindLabel } from '$lib/format';
	import { autoLoad } from '$lib/scroll';
	import { store } from '$lib/store.svelte';
	import type { Confidence, OrphanKind } from '$lib/types';

	let filter = $state<OrphanKind | 'all'>('all');

	const visible = $derived(
		filter === 'all' ? store.orphans : store.orphans.filter((f) => f.kind === filter)
	);

	const kinds = $derived.by(() => {
		const counts = new Map<OrphanKind, number>();
		for (const finding of store.orphans) {
			counts.set(finding.kind, (counts.get(finding.kind) ?? 0) + 1);
		}
		return [...counts.entries()];
	});

	/**
	 * Only verified findings are offered for bulk selection. Selecting a
	 * hundred guesses with one click is exactly the way this feature would
	 * cost somebody their data.
	 */
	function selectVerified() {
		for (const finding of visible) {
			if (finding.confidence === 'certain') store.selected[finding.path] = true;
		}
	}

	function confidenceClass(confidence: Confidence): string {
		return confidence;
	}
</script>

{#if store.orphanTotal === 0 && store.orphanUnavailable.length === 0}
	<div class="empty">
		{#if store.activity === 'findingOrphans'}
			<h2>Looking…</h2>
			<p>Findings appear here as soon as the scan finishes.</p>
		{:else}
			<h2>No orphans found yet</h2>
			<p>Run the orphan scan from the Setup tab.</p>
		{/if}
	</div>
{:else}
	<div class="toolbar">
		<span class="count">
			<strong>{count(store.orphanTotal)}</strong> findings ·
			<strong>{bytes(store.orphanReclaimable)}</strong> reclaimable
		</span>

		<span class="spacer"></span>

		<select bind:value={filter}>
			<option value="all">Everything</option>
			{#each kinds as [kind, n] (kind)}
				<option value={kind}>{orphanKindLabel(kind)} ({n})</option>
			{/each}
		</select>
		<button onclick={selectVerified}>Select verified only</button>
		<button onclick={() => store.clearSelection()} disabled={store.selectedCount === 0}>
			Clear
		</button>
	</div>

	{#if store.orphanUnavailable.length > 0}
		<div class="unavailable">
			{#each store.orphanUnavailable as item (item.kind)}
				<p>
					<strong>{orphanKindLabel(item.kind)} could not be checked.</strong>
					{item.reason}
				</p>
			{/each}
		</div>
	{/if}

	<div class="list">
		{#each visible as finding (finding.path)}
			<label class="row" class:marked={store.isSelected(finding.path)}>
				<input
					type="checkbox"
					checked={store.isSelected(finding.path)}
					onchange={() => store.toggleOrphan(finding.path)}
				/>

				<span class="kind">{orphanKindLabel(finding.kind)}</span>

				<span class="detail">
					<span class="path" title={finding.path}>{finding.path}</span>
					<span class="reason">{finding.reason}</span>
				</span>

				<span class="confidence {confidenceClass(finding.confidence)}">
					{confidenceLabel(finding.confidence)}
				</span>

				<span class="size">{finding.sizeBytes > 0 ? bytes(finding.sizeBytes) : '—'}</span>
				<span class="date">{date(finding.lastModified)}</span>

				<button
					class="reveal"
					title="Show this file in Explorer"
					onclick={(e) => {
						e.preventDefault();
						api.revealInExplorer(finding.path);
					}}
				>
					Open folder
				</button>
			</label>
		{/each}
	</div>

	{#if store.orphanTotal > store.orphans.length}
		<!-- A sentinel rather than a button: the next page starts loading a
	     screen before the user reaches the end, so the list never stops. -->
	<div class="more" use:autoLoad={() => store.loadMoreOrphans()}>
		{#if store.loadingMore}
			<span class="loading">Loading more findings…</span>
		{:else}
			<button onclick={() => store.loadMoreOrphans()}>
				Load the remaining {count(store.orphanTotal - store.orphans.length)} findings
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

	.unavailable {
		margin: 16px 20px 0;
		padding: 10px 14px;
		background: var(--warn-soft);
		border: 1px solid var(--warn);
		border-radius: var(--radius);
		color: var(--warn);
		font-size: 12.5px;
	}

	.unavailable p {
		margin: 4px 0;
	}

	.list {
		padding: 12px 20px 4px;
		display: grid;
		/*
		 * `minmax(0, 1fr)` rather than the default `auto`: a grid column sized
		 * `auto` grows to its content's max-content width, and a wide group
		 * would then drag the whole list past the container and give the page a
		 * horizontal scrollbar.
		 */
		grid-template-columns: minmax(0, 1fr);
		gap: 4px;
	}

	.row {
		display: grid;
		grid-template-columns: auto 150px minmax(0, 1fr) 84px 84px 92px auto;
		align-items: center;
		gap: 12px;
		padding: 8px 12px;
		background: var(--surface);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		cursor: pointer;
	}

	.row:hover {
		background: var(--surface-2);
	}

	.row.marked {
		border-color: var(--danger);
		background: var(--danger-soft);
	}

	.kind {
		font-size: 12px;
		color: var(--text-dim);
	}

	.detail {
		display: grid;
		gap: 1px;
		min-width: 0;
	}

	.path {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-weight: 500;
	}

	.reason {
		font-size: 12px;
		color: var(--text-dim);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.confidence {
		font-size: 11px;
		font-weight: 600;
		text-align: center;
		padding: 1px 6px;
		border-radius: 999px;
	}

	.confidence.certain {
		background: var(--ok-soft);
		color: var(--ok);
	}

	.confidence.likely {
		background: var(--warn-soft);
		color: var(--warn);
	}

	.confidence.suspected {
		background: var(--surface-2);
		color: var(--text-dim);
	}

	.size,
	.date {
		text-align: right;
		font-size: 12px;
		color: var(--text-dim);
		font-variant-numeric: tabular-nums;
	}

	.reveal {
		padding: 2px 8px;
		font-size: 12px;
		white-space: nowrap;
	}

	.more {
		display: flex;
		justify-content: center;
		min-height: 44px;
		align-items: center;
		padding: 12px 20px 24px;
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
