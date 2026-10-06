<script lang="ts">
	let { children } = $props();
</script>

{@render children()}

<style>
	:global(:root) {
		--bg: #f7f7f8;
		--surface: #ffffff;
		--surface-2: #f0f0f2;
		--border: #dcdce0;
		--text: #16161a;
		--text-dim: #62626b;
		--text-faint: #8e8e98;
		--accent: #2f6feb;
		--accent-soft: #e6efff;
		--danger: #c0362c;
		--danger-soft: #fdeceb;
		--warn: #9a6400;
		--warn-soft: #fdf3e0;
		--ok: #1a7a4c;
		--ok-soft: #e6f5ed;
		/* Backdrop behind a letterboxed photo. Distinct from the card so an
		   upright photo reads as matted rather than as a hole in the tile. */
		--photo-mat: #dedee4;
		--radius: 8px;
		--shadow: 0 1px 2px rgb(0 0 0 / 0.06), 0 4px 12px rgb(0 0 0 / 0.04);

		color-scheme: light dark;
		font-family:
			'Segoe UI Variable Text', 'Segoe UI', system-ui, -apple-system, sans-serif;
		font-size: 14px;
		line-height: 1.5;
		color: var(--text);
		background: var(--bg);
		-webkit-font-smoothing: antialiased;
	}

	@media (prefers-color-scheme: dark) {
		:global(:root) {
			--bg: #16161a;
			--surface: #1d1d22;
			--surface-2: #26262d;
			--border: #33333c;
			--text: #ececf0;
			--text-dim: #a2a2ad;
			--text-faint: #71717d;
			--accent: #6fa1ff;
			--accent-soft: #1e2a44;
			--danger: #ff8a80;
			--danger-soft: #3a1f1d;
			--warn: #e0ac4a;
			--warn-soft: #362b16;
			--ok: #6cd39a;
			--ok-soft: #16301f;
			--photo-mat: #0e0e12;
			--shadow: 0 1px 2px rgb(0 0 0 / 0.4), 0 4px 12px rgb(0 0 0 / 0.25);
		}
	}

	:global(*) {
		box-sizing: border-box;
	}

	:global(html),
	:global(body) {
		margin: 0;
		height: 100%;
		overflow: hidden;
	}

	:global(body) {
		background: var(--bg);
		color: var(--text);
	}

	:global(button) {
		font: inherit;
		color: inherit;
		border: 1px solid var(--border);
		background: var(--surface);
		border-radius: var(--radius);
		padding: 6px 12px;
		cursor: pointer;
		transition:
			background 0.12s ease,
			border-color 0.12s ease;
	}

	:global(button:hover:not(:disabled)) {
		background: var(--surface-2);
		border-color: var(--text-faint);
	}

	:global(button:disabled) {
		opacity: 0.45;
		cursor: not-allowed;
	}

	:global(button.primary) {
		background: var(--accent);
		border-color: var(--accent);
		color: #fff;
		font-weight: 600;
	}

	:global(button.primary:hover:not(:disabled)) {
		filter: brightness(1.08);
		background: var(--accent);
		border-color: var(--accent);
	}

	:global(button.danger) {
		background: var(--danger-soft);
		border-color: var(--danger);
		color: var(--danger);
		font-weight: 600;
	}

	:global(input[type='text']),
	:global(input[type='number']),
	:global(select) {
		font: inherit;
		color: inherit;
		background: var(--surface);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		padding: 5px 8px;
	}

	:global(input[type='checkbox']) {
		accent-color: var(--accent);
		width: 15px;
		height: 15px;
		cursor: pointer;
	}

	:global(input[type='range']) {
		accent-color: var(--accent);
	}

	:global(code) {
		font-family: 'Cascadia Mono', 'Consolas', monospace;
		font-size: 0.92em;
	}

	:global(::-webkit-scrollbar) {
		width: 12px;
		height: 12px;
	}

	:global(::-webkit-scrollbar-thumb) {
		background: var(--border);
		border-radius: 6px;
		border: 3px solid transparent;
		background-clip: content-box;
	}

	:global(::-webkit-scrollbar-thumb:hover) {
		background: var(--text-faint);
		background-clip: content-box;
	}
</style>
