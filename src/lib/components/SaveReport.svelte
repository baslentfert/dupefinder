<script lang="ts">
	/**
	 * Writes the findings to a file the user keeps, prints or sends on.
	 *
	 * Deliberately a file rather than a print dialog straight from the app: a
	 * report is usually wanted as evidence of what was there *before* anything
	 * was deleted, and a file survives the app being closed.
	 */

	import { save } from '@tauri-apps/plugin-dialog';
	import { openPath } from '@tauri-apps/plugin-opener';

	import * as api from '$lib/api';
	import { bytes } from '$lib/format';
	import { store } from '$lib/store.svelte';

	let busy = $state(false);
	let result = $state<{ path: string; size: number } | null>(null);
	let error = $state<string | null>(null);

	const hasFindings = $derived(
		store.duplicateTotal > 0 || store.similarTotal > 0 || store.orphanTotal > 0
	);

	function suggestedName(extension: string): string {
		const today = new Date().toISOString().slice(0, 10);
		return `duplicate-finder-${today}.${extension}`;
	}

	async function write(format: api.ReportFormat) {
		error = null;
		result = null;

		const extension = format === 'html' ? 'html' : 'csv';
		const path = await save({
			defaultPath: suggestedName(extension),
			filters: [
				{
					name: format === 'html' ? 'Web page' : 'Spreadsheet',
					extensions: [extension]
				}
			]
		});
		if (!path) return;

		busy = true;
		try {
			// Photo previews are embedded in the HTML so the file stands on its
			// own; a CSV has nowhere to put them.
			const size = await api.saveReport(path, format, format === 'html');
			result = { path, size };
		} catch (caught) {
			error = String(caught);
		} finally {
			busy = false;
		}
	}
</script>

<div class="report">
	<h3>Report</h3>

	<div class="row">
		<button onclick={() => write('html')} disabled={busy || !hasFindings}>
			{busy ? 'Writing…' : 'Save as page'}
		</button>
		<button onclick={() => write('csv')} disabled={busy || !hasFindings}>Save as CSV</button>
	</div>

	{#if !hasFindings}
		<p class="hint">Run a scan first.</p>
	{:else}
		<p class="hint">
			The page is self-contained — previews included — and prints. The CSV is one row per file.
		</p>
	{/if}

	{#if result}
		<p class="done">
			Saved {bytes(result.size)}.
			<button class="link" onclick={() => openPath(result!.path)}>Open</button>
		</p>
	{/if}

	{#if error}
		<p class="failed">{error}</p>
	{/if}
</div>

<style>
	.report {
		padding: 10px 10px 12px;
		border-top: 1px solid var(--border);
	}

	h3 {
		margin: 0 0 8px;
		font-size: 11px;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		color: var(--text-faint);
		font-weight: 600;
	}

	/* Stacked, not side by side: the sidebar is 190px wide and "Save as page"
	   wrapped onto two lines when the buttons shared a row. */
	.row {
		display: grid;
		gap: 5px;
	}

	.row button {
		padding: 5px 8px;
		font-size: 12px;
		white-space: nowrap;
	}

	.hint {
		margin: 8px 0 0;
		font-size: 11.5px;
		color: var(--text-faint);
	}

	.done {
		margin: 8px 0 0;
		font-size: 11.5px;
		color: var(--ok);
	}

	.failed {
		margin: 8px 0 0;
		font-size: 11.5px;
		color: var(--danger);
	}

	.link {
		border: none;
		background: none;
		padding: 0 2px;
		color: var(--accent);
		text-decoration: underline;
		font-size: 11.5px;
	}
</style>
