<script lang="ts">
	import { bytes, count, phaseLabel, shortenPath } from '$lib/format';
	import { store } from '$lib/store.svelte';

	const p = $derived(store.progress);

	// Enumeration has no denominator — the total is only known once the walk
	// finishes — so show a moving count rather than a lying percentage.
	const determinate = $derived(p.total > 0);
	const percent = $derived(determinate ? Math.min(100, (p.done / p.total) * 100) : 0);

	const detail = $derived.by(() => {
		// The orphan detectors cannot know a total — the work is "look at
		// everything and judge it" — so they report a running count and the path
		// they are on instead of a fabricated percentage.
		if (p.examining) {
			return `${count(p.examined)} checked`;
		}
		if (!determinate) {
			return `${count(p.filesSeen)} files, ${bytes(p.bytesSeen)}`;
		}
		if (p.bytesTotal > 0) {
			return `${count(p.done)} of ${count(p.total)} — ${bytes(p.bytesDone)} of ${bytes(p.bytesTotal)}`;
		}
		return `${count(p.done)} of ${count(p.total)}`;
	});
</script>

<div class="panel">
	<div class="head">
		<strong>{p.phase ? phaseLabel(p.phase) : 'Starting…'}</strong>
		<button onclick={() => store.cancel()}>Stop</button>
	</div>

	<div class="track" class:indeterminate={!determinate}>
		<div class="fill" style:width={determinate ? `${percent}%` : undefined}></div>
	</div>

	<p class="detail">{detail}</p>

	{#if p.foundGroups > 0}
		<!-- Duplicates are confirmed the moment a second file lands on a hash, so
		     there is no reason to keep the count to ourselves until the end. -->
		<p class="found">
			<strong>{count(p.foundGroups)}</strong> duplicate groups so far ·
			<strong>{bytes(p.foundBytes)}</strong> reclaimable
		</p>
	{/if}

	{#if p.examining}
		<!-- Shown for the orphan detectors, where a percentage would be a lie.
		     Throttled by the engine to about ten updates a second, which reads
		     as movement without turning into a blur. -->
		<p class="examining" title={p.examining}>{shortenPath(p.examining, 90)}</p>
	{/if}
</div>

<style>
	.panel {
		background: var(--surface);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		padding: 14px 16px;
		margin: 20px;
		box-shadow: var(--shadow);
	}

	.head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
		margin-bottom: 10px;
	}

	.track {
		height: 6px;
		border-radius: 3px;
		background: var(--surface-2);
		overflow: hidden;
	}

	.fill {
		height: 100%;
		background: var(--accent);
		transition: width 0.2s ease;
	}

	.track.indeterminate .fill {
		width: 35%;
		animation: sweep 1.1s ease-in-out infinite;
	}

	@keyframes sweep {
		0% {
			transform: translateX(-100%);
		}
		100% {
			transform: translateX(340%);
		}
	}

	.detail {
		margin: 8px 0 0;
		font-size: 12.5px;
		color: var(--text-dim);
		font-variant-numeric: tabular-nums;
	}

	.found {
		margin: 6px 0 0;
		font-size: 12.5px;
		color: var(--accent);
		font-variant-numeric: tabular-nums;
	}

	.examining {
		margin: 3px 0 0;
		font-family: 'Cascadia Mono', 'Consolas', monospace;
		font-size: 11.5px;
		color: var(--text-faint);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
</style>
