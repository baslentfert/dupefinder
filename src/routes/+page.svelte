<script lang="ts">
	import { onMount, tick } from 'svelte';

	import * as api from '$lib/api';
	import ActionBar from '$lib/components/ActionBar.svelte';
	import DuplicatesView from '$lib/components/DuplicatesView.svelte';
	import FoldersView from '$lib/components/FoldersView.svelte';
	import OrphansView from '$lib/components/OrphansView.svelte';
	import PhotosView from '$lib/components/PhotosView.svelte';
	import ProgressPanel from '$lib/components/ProgressPanel.svelte';
	import SaveReport from '$lib/components/SaveReport.svelte';
	import SetupView from '$lib/components/SetupView.svelte';
	import { bytes, count, duration } from '$lib/format';
	import { store, type Tab } from '$lib/store.svelte';

	onMount(() => {
		void store.init();

		const unsubscribe = api.subscribe({
			onScanEvent: (event) => store.handleEvent(event),
			onOrphanEvent: (event) => store.handleEvent(event),
			onOrphanFinished: () => {
				store.activity = 'idle';
				store.progress.phase = 'done';
				void store.loadOrphans();
			},
			onDuplicatesReady: () => void store.loadDuplicatesEarly(),
			onCancelled: () => {
				store.activity = 'idle';
				store.notice = 'Scan stopped. Partial results were discarded.';
			},
			onFailed: (message) => {
				store.activity = 'idle';
				store.error = message;
			}
		});

		// `subscribe` resolves after the listeners are registered, so the
		// teardown has to wait for it rather than for this callback.
		return () => void unsubscribe.then((off) => off());
	});

	// All three tabs share one scroll container, so without this the photo grid
	// opens at whatever offset the duplicate list was left at — usually deep
	// inside a list the user has not looked at yet. Each tab keeps its own
	// position instead, and returning to one puts it back where it was.
	let content = $state<HTMLElement>();
	let restoring = false;
	const scrollPositions = new Map<Tab, number>();

	function rememberScroll() {
		// Ignore the scroll events the browser fires while we are restoring, or
		// they would overwrite the position we are in the middle of applying.
		if (restoring || !content) return;
		scrollPositions.set(store.tab, content.scrollTop);
	}

	$effect(() => {
		const tab = store.tab;
		if (!content) return;

		restoring = true;
		// The new view has to exist before it can be scrolled.
		void tick().then(() => {
			if (content) content.scrollTop = scrollPositions.get(tab) ?? 0;
			requestAnimationFrame(() => (restoring = false));
		});
	});

	const tabs: { id: Tab; label: string; badge?: () => string }[] = [
		{ id: 'setup', label: 'Setup' },
		{ id: 'duplicates', label: 'Duplicates', badge: () => count(store.duplicateTotal) },
		{ id: 'folders', label: 'Folders', badge: () => count(store.folderTotal) },
		{ id: 'photos', label: 'Photos', badge: () => count(store.similarTotal) },
		{ id: 'orphans', label: 'Orphans', badge: () => count(store.orphanTotal) }
	];

	const scanning = $derived(store.activity !== 'idle');
</script>

<div class="app">
	<aside>
		<div class="brand">
			<span class="mark"></span>
			<span>Duplicate Finder</span>
		</div>

		<nav>
			{#each tabs as tab (tab.id)}
				<button class="tab" class:active={store.tab === tab.id} onclick={() => (store.tab = tab.id)}>
					<span>{tab.label}</span>
					{#if tab.badge && tab.badge() !== '0'}
						<span class="badge">{tab.badge()}</span>
					{/if}
				</button>
			{/each}
		</nav>

		<div class="grow"></div>

		{#if store.summary}
			<div class="stats">
				<h3>Last scan</h3>
				<dl>
					<dt>Files</dt>
					<dd>{count(store.summary.filesSeen)}</dd>
					<dt>Read</dt>
					<dd>{bytes(store.summary.bytesHashed)}</dd>
					{#if store.summary.cacheHits > 0}
						<dt>From cache</dt>
						<dd>{count(store.summary.cacheHits)}</dd>
					{/if}
					<dt>Reclaimable</dt>
					<dd class="highlight">{bytes(store.summary.wastedBytes)}</dd>
					<dt>Took</dt>
					<dd>{duration(store.summary.elapsedMs)}</dd>
					{#if store.summary.skipped > 0}
						<dt>Skipped</dt>
						<dd>{count(store.summary.skipped)}</dd>
					{/if}
				</dl>
			</div>
		{/if}

		<SaveReport />

		<button
			class="primary scan"
			disabled={scanning || store.scanConfig.roots.length === 0}
			onclick={() => store.startScan()}
		>
			{scanning ? 'Scanning…' : 'Start scan'}
		</button>
	</aside>

	<main>
		{#if store.error}
			<div class="banner error">
				<span>{store.error}</span>
				<button onclick={() => (store.error = null)}>Dismiss</button>
			</div>
		{/if}

		{#if store.notice}
			<div class="banner notice">
				<span>{store.notice}</span>
				<button onclick={() => (store.notice = null)}>Dismiss</button>
			</div>
		{/if}

		{#if scanning}
			<ProgressPanel />
		{/if}

		<div class="content" bind:this={content} onscroll={rememberScroll}>
			{#if store.tab === 'setup'}
				<SetupView />
			{:else if store.tab === 'duplicates'}
				<DuplicatesView />
			{:else if store.tab === 'folders'}
				<FoldersView />
			{:else if store.tab === 'photos'}
				<PhotosView />
			{:else}
				<OrphansView />
			{/if}
		</div>

		<ActionBar />
	</main>
</div>

<style>
	.app {
		display: grid;
		grid-template-columns: 216px 1fr;
		height: 100vh;
	}

	aside {
		display: flex;
		flex-direction: column;
		gap: 4px;
		padding: 14px 12px;
		background: var(--surface);
		border-right: 1px solid var(--border);
	}

	.brand {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 4px 8px 14px;
		font-weight: 600;
	}

	.mark {
		width: 14px;
		height: 14px;
		border-radius: 4px;
		background: var(--accent);
		box-shadow: 4px 4px 0 -1px var(--accent-soft);
	}

	nav {
		display: grid;
		gap: 2px;
	}

	.tab {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 8px;
		border: none;
		background: none;
		text-align: left;
		padding: 7px 10px;
		border-radius: 6px;
		color: var(--text-dim);
	}

	.tab:hover {
		background: var(--surface-2);
	}

	.tab.active {
		background: var(--accent-soft);
		color: var(--accent);
		font-weight: 600;
	}

	.badge {
		font-size: 11px;
		font-variant-numeric: tabular-nums;
		background: var(--surface-2);
		border-radius: 999px;
		padding: 0 7px;
		color: var(--text-dim);
	}

	.tab.active .badge {
		background: var(--accent);
		color: #fff;
	}

	.grow {
		flex: 1;
	}

	.stats {
		padding: 10px 10px 12px;
		border-top: 1px solid var(--border);
		margin-bottom: 10px;
	}

	.stats h3 {
		margin: 0 0 8px;
		font-size: 11px;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		color: var(--text-faint);
		font-weight: 600;
	}

	dl {
		display: grid;
		grid-template-columns: 1fr auto;
		gap: 3px 8px;
		margin: 0;
		font-size: 12.5px;
	}

	dt {
		color: var(--text-dim);
	}

	dd {
		margin: 0;
		text-align: right;
		font-variant-numeric: tabular-nums;
	}

	dd.highlight {
		color: var(--accent);
		font-weight: 600;
	}

	.scan {
		padding: 9px;
	}

	main {
		display: flex;
		flex-direction: column;
		min-width: 0;
		overflow: hidden;
	}

	.content {
		flex: 1;
		overflow: auto;
		min-height: 0;
	}

	.banner {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
		padding: 9px 20px;
	}

	.banner.error {
		background: var(--danger-soft);
		color: var(--danger);
	}

	.banner.notice {
		background: var(--warn-soft);
		color: var(--warn);
	}
</style>
