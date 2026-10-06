/** Typed wrappers around the Tauri commands. */

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

import type {
	ActionReport,
	Activity,
	DuplicatePage,
	FolderPage,
	GroupPlan,
	KeepRule,
	OrphanConfig,
	OrphanPage,
	OrphanPlan,
	Plan,
	RestorePointAvailability,
	ScanConfig,
	ScanEvent,
	ScanStrategy,
	SimilarPage,
	Summary,
	SystemInfo,
	Violation
} from './types';

/** Marks time-to-interactive in the backend log. */
export function frontendReady(): Promise<void> {
	return invoke('frontend_ready');
}

export function systemInfo(): Promise<SystemInfo> {
	return invoke('system_info');
}

export function startScan(config: ScanConfig): Promise<void> {
	return invoke('start_scan', { config });
}

export function startOrphanScan(config: OrphanConfig): Promise<void> {
	return invoke('start_orphan_scan', { config });
}

export function cancelScan(): Promise<void> {
	return invoke('cancel_scan');
}

export function activity(): Promise<Activity> {
	return invoke('activity');
}

export function duplicatePage(offset: number, limit: number): Promise<DuplicatePage> {
	return invoke('duplicate_page', { offset, limit });
}

export function similarPage(offset: number, limit: number): Promise<SimilarPage> {
	return invoke('similar_page', { offset, limit });
}

export function folderPage(offset: number, limit: number): Promise<FolderPage> {
	return invoke('folder_page', { offset, limit });
}

/**
 * File-level plans for keeping one copy of every duplicated folder.
 *
 * Returns the same shape as the duplicate suggestions: the folder view is a
 * way to decide, not a separate way to delete.
 */
export function suggestFolderPlan(rule: KeepRule): Promise<GroupPlan[]> {
	return invoke('suggest_folder_plan', { rule });
}

/** Plans for keeping one named copy of one duplicated folder. */
export function folderPlan(groupId: number, keep: string): Promise<GroupPlan[]> {
	return invoke('folder_plan', { groupId, keep });
}

export function orphanPage(offset: number, limit: number): Promise<OrphanPage> {
	return invoke('orphan_page', { offset, limit });
}

export function suggestDuplicatePlan(rule: KeepRule): Promise<GroupPlan[]> {
	return invoke('suggest_duplicate_plan', { rule });
}

export function suggestSimilarPlan(rule: KeepRule): Promise<GroupPlan[]> {
	return invoke('suggest_similar_plan', { rule });
}

export function checkPlan(plan: Plan): Promise<Violation[]> {
	return invoke('check_plan', { plan });
}

export function restorePointAvailability(): Promise<RestorePointAvailability> {
	return invoke('restore_point_availability');
}

export function checkOrphanPlan(plan: OrphanPlan): Promise<Violation[]> {
	return invoke('check_orphan_plan', { plan });
}

/**
 * Both apply commands reject with a `Violation[]` rather than a string when
 * the plan is refused, so callers must narrow before showing the error.
 */
export function applyPlan(plan: Plan): Promise<ActionReport> {
	return invoke('apply_plan', { plan });
}

export function applyOrphanPlan(plan: OrphanPlan): Promise<ActionReport> {
	return invoke('apply_orphan_plan', { plan });
}

export function revealInExplorer(path: string): Promise<void> {
	return invoke('reveal_in_explorer', { request: { path } });
}

export function resolvedStrategy(config: ScanConfig): Promise<ScanStrategy> {
	return invoke('resolved_strategy', { config });
}

export function clearResults(): Promise<void> {
	return invoke('clear_results');
}

export type ReportFormat = 'html' | 'csv';

/**
 * Write the findings to `path`. Returns the size of the file written.
 *
 * Rendering an HTML report embeds a preview per photo, so it is slower than a
 * CSV and the caller should show that it is working.
 */
export function saveReport(
	path: string,
	format: ReportFormat,
	includeThumbnails: boolean
): Promise<number> {
	return invoke('save_report', { path, format, includeThumbnails });
}

export function exportReport(): Promise<unknown> {
	return invoke('export_report');
}

/** True when a rejected `applyPlan` carried refusals rather than an error. */
export function isViolations(error: unknown): error is Violation[] {
	return Array.isArray(error) && error.every((v) => typeof v?.reason === 'string');
}

export interface EngineEvents {
	onScanEvent?: (event: ScanEvent) => void;
	onOrphanEvent?: (event: ScanEvent) => void;
	onOrphanFinished?: (payload: { findings: number; reclaimableBytes: number }) => void;
	/** Exact duplicates are final; the image half is still running. */
	onDuplicatesReady?: () => void;
	onCancelled?: () => void;
	onFailed?: (message: string) => void;
}

/**
 * Subscribe to every engine channel at once.
 *
 * Returns a single unsubscribe function; call it on teardown or the listeners
 * outlive the component and fire against stale state.
 */
export async function subscribe(handlers: EngineEvents): Promise<UnlistenFn> {
	const unlisteners: UnlistenFn[] = [];

	if (handlers.onScanEvent) {
		const handler = handlers.onScanEvent;
		unlisteners.push(await listen<ScanEvent>('scan-event', (e) => handler(e.payload)));
	}
	if (handlers.onOrphanEvent) {
		const handler = handlers.onOrphanEvent;
		unlisteners.push(await listen<ScanEvent>('orphan-event', (e) => handler(e.payload)));
	}
	if (handlers.onOrphanFinished) {
		const handler = handlers.onOrphanFinished;
		unlisteners.push(
			await listen<{ findings: number; reclaimableBytes: number }>('orphan-finished', (e) =>
				handler(e.payload)
			)
		);
	}
	if (handlers.onDuplicatesReady) {
		const handler = handlers.onDuplicatesReady;
		unlisteners.push(await listen('duplicates-ready', () => handler()));
	}
	if (handlers.onCancelled) {
		const handler = handlers.onCancelled;
		unlisteners.push(await listen('scan-cancelled', () => handler()));
	}
	if (handlers.onFailed) {
		const handler = handlers.onFailed;
		unlisteners.push(await listen<string>('scan-failed', (e) => handler(e.payload)));
	}

	return () => unlisteners.forEach((off) => off());
}

/** URL for the preview of `path`, served by the `thumb` protocol. */
export function thumbnailUrl(path: string, size = 224): string {
	const query = `path=${encodeURIComponent(path)}&size=${size}`;
	// Windows serves custom schemes over `http://<scheme>.localhost`; every
	// other platform uses the scheme directly.
	const base =
		navigator.userAgent.includes('Windows') ?
			'http://thumb.localhost/'
		:	'thumb://localhost/';
	return `${base}?${query}`;
}

export type { Summary };
