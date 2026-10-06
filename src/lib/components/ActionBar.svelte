<script lang="ts">
	import { open } from '@tauri-apps/plugin-dialog';

	import * as api from '$lib/api';
	import { bytes, count, shortenPath } from '$lib/format';
	import { store } from '$lib/store.svelte';
	import type { Action, ActionReport, RestorePointAvailability, Violation } from '$lib/types';

	type Kind = 'recycle' | 'moveTo' | 'replaceWithHardLink' | 'deletePermanently';

	let kind = $state<Kind>('recycle');
	let destination = $state<string | null>(null);
	let confirming = $state(false);
	let violations = $state<Violation[]>([]);
	let report = $state<ActionReport | null>(null);
	let failure = $state<string | null>(null);

	const onOrphans = $derived(store.tab === 'orphans');

	// Offered for orphans only. A restore point covers programs and the
	// registry — which is what the installer cache and leftover app data are —
	// but not user data, so suggesting it beside a photo delete would be a
	// promise the feature cannot keep.
	let restorePoint = $state(true);
	let restoreState = $state<RestorePointAvailability | null>(null);

	$effect(() => {
		if (!onOrphans || restoreState !== null) return;
		void api.restorePointAvailability().then((state) => (restoreState = state));
	});

	const restoreReady = $derived(restoreState === 'ready');

	// Only worth asking about when the selection holds something a restore
	// point actually covers.
	const restoreRelevant = $derived(onOrphans && store.selectionTouchesPrograms());

	const restoreNote = $derived.by(() => {
		switch (restoreState) {
			case 'ready':
				return 'Windows will be asked for a restore point first.';
			case 'needsElevation':
				// Not "untick this": the box is disabled, so there is nothing to
				// untick. Say what will actually happen instead.
				return 'Needs administrator rights. Restart the app as administrator to get one — otherwise the cleanup goes ahead without it.';
			case 'protectionDisabled':
				return 'System protection is off for this drive, so Windows cannot make one.';
			case 'unsupported':
				return 'Restore points are a Windows feature.';
			default:
				return 'Checking…';
		}
	});

	// Hard links replace a duplicate with a pointer at the copy being kept.
	// An orphan has nothing to point at.
	const canHardLink = $derived(!onOrphans);

	const action = $derived.by((): Action => {
		switch (kind) {
			case 'moveTo':
				return { type: 'moveTo', destination: destination ?? '' };
			case 'replaceWithHardLink':
				return { type: 'replaceWithHardLink' };
			case 'deletePermanently':
				return { type: 'deletePermanently' };
			default:
				return { type: 'recycle' };
		}
	});

	const reversible = $derived(kind === 'recycle' || kind === 'moveTo');
	const ready = $derived(kind !== 'moveTo' || !!destination);

	async function chooseDestination() {
		const chosen = await open({
			directory: true,
			multiple: false,
			title: 'Where should the files be moved?'
		});
		if (typeof chosen === 'string') destination = chosen;
	}

	/** Ask the engine what it would refuse, before showing a confirmation. */
	async function review() {
		failure = null;
		report = null;
		store.busy = true;

		try {
			violations =
				onOrphans ?
					await api.checkOrphanPlan({
						action,
						paths: store.selectedOrphanPaths(),
						createRestorePoint: restorePoint && restoreReady
					})
				:	await api.checkPlan({ action, groups: store.buildGroupPlans() });
			confirming = true;
		} catch (error) {
			failure = String(error);
		} finally {
			store.busy = false;
		}
	}

	async function apply() {
		failure = null;
		store.busy = true;

		try {
			report =
				onOrphans ?
					await api.applyOrphanPlan({
						action,
						paths: store.selectedOrphanPaths(),
						createRestorePoint: restorePoint && restoreReady
					})
				:	await api.applyPlan({ action, groups: store.buildGroupPlans() });

			store.clearSelection();
			confirming = false;

			// The backend already pruned what it removed; re-read so the lists
			// match.
			if (onOrphans) await store.loadOrphans();
			else await store.loadResults();
		} catch (error) {
			if (api.isViolations(error)) {
				violations = error;
				failure = 'The plan was refused. Nothing has been changed.';
			} else {
				failure = String(error);
			}
		} finally {
			store.busy = false;
		}
	}

	const verb = $derived(
		{
			recycle: 'Move to the recycle bin',
			moveTo: 'Move to a folder',
			replaceWithHardLink: 'Replace with hard links',
			deletePermanently: 'Delete permanently'
		}[kind]
	);
</script>

{#if report}
	<div class="bar done">
		<span>
			<strong>{count(report.succeeded)}</strong> files handled,
			<strong>{bytes(report.bytesFreed)}</strong> freed.
			{#if report.failed > 0}
				<span class="bad">{count(report.failed)} failed.</span>
			{/if}
		</span>

		{#if report.failed > 0}
			<details>
				<summary>What failed</summary>
				<ul>
					{#each report.outcomes.filter((o) => !o.ok) as outcome (outcome.path)}
						<li><code>{shortenPath(outcome.path)}</code> — {outcome.error}</li>
					{/each}
				</ul>
			</details>
		{/if}

		<span class="spacer"></span>
		<button onclick={() => (report = null)}>Dismiss</button>
	</div>
{/if}

{#if store.selectedCount > 0}
	<div class="bar">
		{#if confirming}
			<div class="confirm">
				<div class="head">
					<strong>{verb}</strong>: {count(store.selectedCount)} files, {bytes(store.selectedBytes)}.
					{#if !reversible}
						<span class="bad">This cannot be undone.</span>
					{/if}
				</div>

				{#if restoreRelevant}
					<label class="restore" class:unavailable={!restoreReady}>
						<!-- Shows the *effective* state, not the preference: a disabled
						     box that still looks ticked would claim a restore point is
						     coming when the plan is about to be sent without one. -->
						<input
							type="checkbox"
							checked={restorePoint && restoreReady}
							disabled={!restoreReady}
							onchange={(event) => (restorePoint = event.currentTarget.checked)}
						/>
						<span>
							<strong>Create a system restore point first</strong>
							<em>
								{restoreNote}
								<!-- Said plainly, because believing otherwise is exactly how
								     someone loses a file they thought was covered. -->
								A restore point protects programs and the registry, not your own
								files — those go to the recycle bin.
							</em>
						</span>
					</label>
				{/if}

				{#if violations.length > 0}
					<div class="violations">
						<strong>{count(violations.length)} refused:</strong>
						<ul>
							{#each violations.slice(0, 6) as violation, i (i)}
								<li>
									{#if violation.path}<code>{shortenPath(violation.path, 50)}</code> — {/if}
									{violation.reason}
								</li>
							{/each}
							{#if violations.length > 6}
								<li>…and {count(violations.length - 6)} more.</li>
							{/if}
						</ul>
						<p>Nothing runs until every one of these is resolved.</p>
					</div>
				{/if}

				<div class="actions">
					<button onclick={() => (confirming = false)}>Back</button>
					<button
						class={reversible ? 'primary' : 'danger'}
						disabled={violations.length > 0 || store.busy}
						onclick={apply}
					>
						{store.busy ? 'Working…' : `Yes, ${verb.toLowerCase()}`}
					</button>
				</div>
			</div>
		{:else}
			<span class="summary">
				<strong>{count(store.selectedCount)}</strong> selected · {bytes(store.selectedBytes)}
			</span>

			<span class="spacer"></span>

			<select bind:value={kind}>
				<option value="recycle">Move to the recycle bin</option>
				<option value="moveTo">Move to a folder…</option>
				{#if canHardLink}
					<option value="replaceWithHardLink">Replace with a hard link</option>
				{/if}
				<option value="deletePermanently">Delete permanently</option>
			</select>

			{#if kind === 'moveTo'}
				<button onclick={chooseDestination}>
					{destination ? shortenPath(destination, 34) : 'Choose folder…'}
				</button>
			{/if}

			{#if kind === 'replaceWithHardLink'}
				<span class="note">Frees the space, every path keeps working. Same drive only.</span>
			{/if}

			<button onclick={() => store.clearSelection()}>Clear</button>
			<button class="primary" disabled={!ready || store.busy} onclick={review}>Continue</button>
		{/if}
	</div>
{/if}

{#if failure}
	<div class="bar error">
		<span>{failure}</span>
		<span class="spacer"></span>
		<button onclick={() => (failure = null)}>Dismiss</button>
	</div>
{/if}

<style>
	.bar {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 10px 20px;
		background: var(--surface);
		border-top: 1px solid var(--border);
		box-shadow: 0 -4px 12px rgb(0 0 0 / 0.06);
	}

	.bar.done {
		background: var(--ok-soft);
		color: var(--ok);
	}

	.bar.error {
		background: var(--danger-soft);
		color: var(--danger);
	}

	.spacer {
		flex: 1;
	}

	.summary {
		color: var(--text-dim);
	}

	.note {
		font-size: 12px;
		color: var(--text-dim);
	}

	.bad {
		color: var(--danger);
		font-weight: 600;
	}

	.confirm {
		display: grid;
		gap: 10px;
		width: 100%;
	}

	.head {
		font-size: 14px;
	}

	.restore {
		display: flex;
		gap: 10px;
		align-items: flex-start;
		padding: 9px 12px;
		background: var(--surface-2);
		border-radius: var(--radius);
		cursor: pointer;
	}

	.restore.unavailable {
		cursor: default;
		opacity: 0.75;
	}

	.restore input {
		margin-top: 2px;
		flex: none;
	}

	.restore strong {
		display: block;
		font-size: 13px;
	}

	.restore em {
		display: block;
		font-style: normal;
		font-size: 12px;
		color: var(--text-dim);
		margin-top: 2px;
	}

	.violations {
		background: var(--danger-soft);
		border: 1px solid var(--danger);
		border-radius: var(--radius);
		padding: 10px 14px;
		font-size: 12.5px;
		max-height: 180px;
		overflow: auto;
	}

	.violations ul,
	details ul {
		margin: 6px 0;
		padding-left: 18px;
	}

	.violations p {
		margin: 6px 0 0;
	}

	.actions {
		display: flex;
		gap: 8px;
		justify-content: flex-end;
	}

	details {
		font-size: 12.5px;
		max-height: 140px;
		overflow: auto;
	}

	summary {
		cursor: pointer;
	}
</style>
