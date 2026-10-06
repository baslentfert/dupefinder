<script lang="ts">
	import { untrack } from 'svelte';
	import { open } from '@tauri-apps/plugin-dialog';

	import * as api from '$lib/api';
	import { bytes } from '$lib/format';
	import { store } from '$lib/store.svelte';
	import type { ScanStrategy } from '$lib/types';

	let strategy = $state<ScanStrategy>('auto');

	// The chosen strategy depends on the roots, the requested strategy and
	// whether the process is elevated, so ask the engine rather than guessing.
	//
	// Only those two fields are read as dependencies; the rest of the config is
	// read inside `untrack`, or every checkbox and every keystroke in the size
	// field would fire another round trip.
	$effect(() => {
		const roots = store.scanConfig.roots.map((root) => root);
		const requested = store.scanConfig.strategy;

		const config = untrack(() => ({
			...$state.snapshot(store.scanConfig),
			roots,
			strategy: requested
		}));

		void api.resolvedStrategy(config).then((resolved) => (strategy = resolved));
	});

	async function addFolder() {
		const chosen = await open({ directory: true, multiple: true, title: 'Add a folder to scan' });
		if (!chosen) return;

		const paths = Array.isArray(chosen) ? chosen : [chosen];
		for (const path of paths) {
			if (!store.scanConfig.roots.includes(path)) store.scanConfig.roots.push(path);
		}
	}

	function toggleRoot(path: string) {
		const index = store.scanConfig.roots.indexOf(path);
		if (index >= 0) store.scanConfig.roots.splice(index, 1);
		else store.scanConfig.roots.push(path);
	}

	function removeRoot(path: string) {
		store.scanConfig.roots = store.scanConfig.roots.filter((r) => r !== path);
	}

	const thresholdHint = $derived.by(() => {
		const t = store.scanConfig.similarityThreshold;
		if (t <= 4) return 'Only near-identical copies: re-saves and format changes.';
		if (t <= 12) return 'Resizes, re-encodes and messaging-app copies of the same photo.';
		if (t <= 20) return 'Also burst shots and lightly edited versions.';
		return 'Very loose — expect unrelated photos to be grouped together.';
	});
</script>

<div class="setup">
	<section>
		<h2>Where to look</h2>

		<div class="roots">
			{#each store.scanConfig.roots as root (root)}
				<span class="chip">
					<code>{root}</code>
					<button class="chip-x" onclick={() => removeRoot(root)} title="Remove">×</button>
				</span>
			{:else}
				<p class="empty">Nothing selected yet.</p>
			{/each}
		</div>

		<div class="row">
			<button onclick={addFolder}>Add folder…</button>
		</div>

		{#if store.system}
			<h3>Suggestions</h3>
			<div class="row wrap">
				{#each store.system.suggestedRoots as suggestion (suggestion.path)}
					<button
						class:active={store.scanConfig.roots.includes(suggestion.path)}
						onclick={() => toggleRoot(suggestion.path)}
					>
						{suggestion.label}
					</button>
				{/each}
			</div>

			<h3>Drives</h3>
			<div class="drives">
				{#each store.system.volumes as volume (volume.letter)}
					{@const path = `${volume.letter}:\\`}
					<button
						class="drive"
						class:active={store.scanConfig.roots.includes(path)}
						onclick={() => toggleRoot(path)}
					>
						<span class="drive-letter">{volume.letter}:</span>
						<span class="drive-name">
							{volume.label || (volume.isFixed ? 'Local disk' : 'Removable disk')}
						</span>
						<span class="drive-space">
							{bytes(volume.totalBytes - volume.freeBytes)} of {bytes(volume.totalBytes)} used
							· {volume.fileSystem}
						</span>
						<span class="bar">
							<span
								class="bar-fill"
								style:width="{volume.totalBytes > 0
									? ((volume.totalBytes - volume.freeBytes) / volume.totalBytes) * 100
									: 0}%"
							></span>
						</span>
					</button>
				{/each}
			</div>
		{/if}
	</section>

	<section>
		<h2>What to look for</h2>

		<label class="check">
			<input type="checkbox" bind:checked={store.scanConfig.detectors.exact} />
			<span>
				<strong>Identical files</strong>
				<em>Byte-for-byte duplicates of any type.</em>
			</span>
		</label>

		<label class="check">
			<input type="checkbox" bind:checked={store.scanConfig.detectors.similarImages} />
			<span>
				<strong>Look-alike photos</strong>
				<em>
					Resizes, re-encodes and edits of the same picture. Reads the preview embedded in HEIC
					and JPEG files, so iPhone photos do not have to be fully decoded.
				</em>
			</span>
		</label>

		{#if store.scanConfig.detectors.similarImages}
			<div class="slider">
				<label for="threshold">
					How alike is alike? <strong>{store.scanConfig.similarityThreshold} of 64 bits</strong>
				</label>
				<input
					id="threshold"
					type="range"
					min="2"
					max="24"
					bind:value={store.scanConfig.similarityThreshold}
				/>
				<p class="hint">{thresholdHint}</p>
			</div>
		{/if}
	</section>

	<section>
		<h2>Options</h2>

		<label class="check">
			<input type="checkbox" bind:checked={store.scanConfig.skipSystemPaths} />
			<span>
				<strong>Skip Windows and Program Files</strong>
				<em>
					These legitimately contain thousands of identical files. Deleting anything there is
					blocked regardless of this setting.
				</em>
			</span>
		</label>

		<label class="check">
			<input type="checkbox" bind:checked={store.scanConfig.skipCloudPlaceholders} />
			<span>
				<strong>Skip files stored only in the cloud</strong>
				<em>
					Comparing a file means reading it, and reading a OneDrive or Dropbox placeholder makes
					Windows download it first. Turning this off can pull down your whole cloud folder.
				</em>
			</span>
		</label>

		<label class="check">
			<input type="checkbox" bind:checked={store.scanConfig.includeHidden} />
			<span><strong>Include hidden files</strong></span>
		</label>

		<label class="check">
			<input type="checkbox" bind:checked={store.scanConfig.verifyBytes} />
			<span>
				<strong>Compare byte for byte before reporting</strong>
				<em>
					Reads every candidate a second time. A BLAKE3 collision is not a practical risk, so
					this is off by default.
				</em>
			</span>
		</label>

		<label class="field">
			<span>Ignore files smaller than</span>
			<input type="number" min="0" step="1" bind:value={store.scanConfig.minSize} />
			<span class="unit">bytes</span>
		</label>

		<div class="strategy">
			{#if strategy === 'mft'}
				<span class="badge ok">Fast path</span>
				Reading the NTFS master file table directly — whole drives in seconds.
			{:else if store.system && !store.system.elevated}
				<span class="badge">Directory walk</span>
				Running as administrator would let whole drives be listed from the NTFS master file table
				instead, which is far faster.
			{:else}
				<span class="badge">Directory walk</span>
				The fast path only applies when whole drives are selected.
			{/if}
		</div>
	</section>

	<section>
		<h2>Orphans</h2>
		<p class="hint">
			Leftovers that nothing owns any more. Scanned separately, because it looks at the registry and
			the shell rather than at file contents.
		</p>

		<label class="check">
			<input type="checkbox" bind:checked={store.orphanConfig.brokenShortcuts} />
			<span><strong>Broken shortcuts</strong><em>.lnk files pointing at something that is gone.</em></span>
		</label>

		<label class="check">
			<input type="checkbox" bind:checked={store.orphanConfig.emptyFolders} />
			<span><strong>Empty folders</strong><em>Including nests of folders that only contain each other.</em></span>
		</label>

		<label class="check">
			<input type="checkbox" bind:checked={store.orphanConfig.leftoverAppdata} />
			<span>
				<strong>Leftover application data</strong>
				<em>
					Folders in AppData and ProgramData with no matching installed program. A guess, always
					shown as merely suspected.
				</em>
			</span>
		</label>

		<label class="check">
			<input type="checkbox" bind:checked={store.orphanConfig.orphanedInstaller} />
			<span>
				<strong>Orphaned installer packages</strong>
				<em>
					.msi and .msp files in the Windows installer cache that no installed product
					references. Often several gigabytes. Needs administrator rights to read reliably.
				</em>
			</span>
		</label>

		<button onclick={() => store.startOrphanScan()} disabled={store.activity !== 'idle'}>
			Look for orphans
		</button>
	</section>
</div>

<style>
	/*
	 * Columns rather than a grid: the drive list is far taller than the other
	 * panels, and grid rows would leave the rest of its row empty. Flowing the
	 * sections lets the short ones stack up beside it.
	 */
	.setup {
		columns: 360px 2;
		column-gap: 16px;
		padding: 20px;
	}

	section {
		background: var(--surface);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		padding: 16px 18px;
		margin-bottom: 16px;
		box-shadow: var(--shadow);
		break-inside: avoid;
	}

	h2 {
		margin: 0 0 12px;
		font-size: 15px;
		font-weight: 600;
	}

	h3 {
		margin: 18px 0 8px;
		font-size: 12px;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		color: var(--text-faint);
	}

	.roots {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		margin-bottom: 10px;
	}

	.chip {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		background: var(--accent-soft);
		border-radius: 999px;
		padding: 3px 6px 3px 10px;
		max-width: 100%;
	}

	.chip code {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.chip-x {
		border: none;
		background: none;
		padding: 0 4px;
		line-height: 1;
		font-size: 16px;
		color: var(--text-dim);
	}

	.row {
		display: flex;
		gap: 8px;
	}

	.row.wrap {
		flex-wrap: wrap;
	}

	button.active {
		background: var(--accent-soft);
		border-color: var(--accent);
		color: var(--accent);
		font-weight: 600;
	}

	.drives {
		display: grid;
		gap: 8px;
	}

	.drive {
		display: grid;
		grid-template-columns: auto 1fr;
		grid-template-areas:
			'letter name'
			'letter space'
			'bar bar';
		gap: 2px 10px;
		text-align: left;
		padding: 10px 12px;
	}

	.drive-letter {
		grid-area: letter;
		align-self: center;
		font-size: 18px;
		font-weight: 600;
		color: var(--text-dim);
	}

	.drive-name {
		grid-area: name;
		font-weight: 600;
	}

	.drive-space {
		grid-area: space;
		font-size: 12px;
		color: var(--text-dim);
	}

	.bar {
		grid-area: bar;
		margin-top: 6px;
		height: 4px;
		border-radius: 2px;
		background: var(--surface-2);
		overflow: hidden;
	}

	.bar-fill {
		display: block;
		height: 100%;
		background: var(--accent);
	}

	.check {
		display: flex;
		gap: 10px;
		align-items: flex-start;
		padding: 8px 0;
		cursor: pointer;
	}

	.check input {
		margin-top: 2px;
		flex: none;
	}

	.check strong {
		display: block;
		font-weight: 600;
	}

	.check em {
		display: block;
		font-style: normal;
		font-size: 12.5px;
		color: var(--text-dim);
		margin-top: 2px;
	}

	.field {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 8px 0;
	}

	.field input {
		width: 110px;
	}

	.unit,
	.hint {
		color: var(--text-dim);
		font-size: 12.5px;
	}

	.hint {
		margin: 6px 0 12px;
	}

	.slider {
		margin: 6px 0 0;
		padding: 12px;
		background: var(--surface-2);
		border-radius: var(--radius);
	}

	.slider input {
		width: 100%;
		margin: 8px 0 0;
	}

	.strategy {
		margin-top: 14px;
		padding: 10px 12px;
		background: var(--surface-2);
		border-radius: var(--radius);
		font-size: 12.5px;
		color: var(--text-dim);
	}

	.badge {
		display: inline-block;
		margin-right: 6px;
		padding: 1px 7px;
		border-radius: 999px;
		background: var(--border);
		color: var(--text);
		font-size: 11px;
		font-weight: 600;
	}

	.badge.ok {
		background: var(--ok-soft);
		color: var(--ok);
	}

	.empty {
		color: var(--text-faint);
		margin: 0;
	}
</style>
