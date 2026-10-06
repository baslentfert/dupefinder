/**
 * Application state.
 *
 * Selection is modelled as a set of paths *marked for removal*, not as a
 * chosen keeper. That is the operation the user is actually performing, and it
 * makes the safety rule expressible directly: a group must always have at
 * least one unselected member, and {@link Store.toggle} refuses the click that
 * would break it rather than silently dropping the group at apply time.
 */

import * as api from './api';
import {
	defaultOrphanConfig,
	defaultScanConfig,
	type Activity,
	type DetectorUnavailable,
	type DuplicateGroup,
	type FolderGroup,
	type FileEntry,
	type OrphanConfig,
	type OrphanFinding,
	type Phase,
	type ScanConfig,
	type ScanEvent,
	type SimilarGroup,
	type Summary,
	type SystemInfo
} from './types';

/** Groups fetched per request, and per "load more" click. */
const PAGE_SIZE = 200;

/** Skipped-file notices retained for the report. Older ones are dropped. */
const MAX_SKIPPED = 500;

export interface Progress {
	phase: Phase | null;
	done: number;
	total: number;
	bytesDone: number;
	bytesTotal: number;
	filesSeen: number;
	bytesSeen: number;
	/** What the engine is looking at right now, for work with no total. */
	examining: string | null;
	examined: number;
	/** Confirmed duplicates so far, while hashing is still running. */
	foundGroups: number;
	foundBytes: number;
}

export type Tab = 'setup' | 'duplicates' | 'folders' | 'photos' | 'orphans';

function emptyProgress(): Progress {
	return {
		phase: null,
		done: 0,
		total: 0,
		bytesDone: 0,
		bytesTotal: 0,
		filesSeen: 0,
		bytesSeen: 0,
		examining: null,
		examined: 0,
		foundGroups: 0,
		foundBytes: 0
	};
}

export class Store {
	system = $state<SystemInfo | null>(null);
	scanConfig = $state<ScanConfig>(defaultScanConfig());
	orphanConfig = $state<OrphanConfig>(defaultOrphanConfig());

	tab = $state<Tab>('setup');
	activity = $state<Activity>('idle');
	progress = $state<Progress>(emptyProgress());
	summary = $state<Summary | null>(null);
	error = $state<string | null>(null);
	notice = $state<string | null>(null);
	skipped = $state<{ path: string; reason: string }[]>([]);

	duplicates = $state<DuplicateGroup[]>([]);
	duplicateTotal = $state(0);
	folders = $state<FolderGroup[]>([]);
	folderTotal = $state(0);
	folderReclaimable = $state(0);
	similar = $state<SimilarGroup[]>([]);
	similarTotal = $state(0);
	orphans = $state<OrphanFinding[]>([]);
	orphanTotal = $state(0);
	orphanUnavailable = $state<DetectorUnavailable[]>([]);
	orphanReclaimable = $state(0);

	/** Paths marked for removal, across every tab. */
	selected = $state<Record<string, true>>({});

	busy = $state(false);
	/** Guards the auto-loading sentinel from firing while a page is in flight. */
	loadingMore = $state(false);

	selectedCount = $derived(Object.keys(this.selected).length);

	selectedBytes = $derived.by(() => {
		let total = 0;
		for (const group of this.duplicates) {
			for (const file of group.files) {
				if (this.selected[file.path]) total += file.size;
			}
		}
		for (const group of this.similar) {
			for (const image of group.images) {
				if (this.selected[image.file.path]) total += image.file.size;
			}
		}
		for (const finding of this.orphans) {
			if (this.selected[finding.path]) total += finding.sizeBytes;
		}
		return total;
	});

	async init() {
		// Reported before anything else, so a slow drive probe cannot be
		// mistaken for a slow UI.
		void api.frontendReady();

		try {
			this.system = await api.systemInfo();
			// Start with the folders where duplicates actually pile up, rather
			// than with nothing selected.
			this.scanConfig.roots = this.system.suggestedRoots
				.filter((r) => r.label === 'Pictures' || r.label === 'Downloads')
				.map((r) => r.path);
			this.orphanConfig.roots = this.system.suggestedRoots.map((r) => r.path);
		} catch (error) {
			this.error = String(error);
		}
	}

	handleEvent(event: ScanEvent) {
		switch (event.kind) {
			case 'phaseStarted':
				this.progress.phase = event.phase;
				// Each phase counts its own work; carrying the old numbers over
				// would show a bar jumping backwards.
				this.progress.done = 0;
				this.progress.total = 0;
				this.progress.bytesDone = 0;
				this.progress.bytesTotal = 0;
				this.progress.examining = null;
				this.progress.examined = 0;
				break;

			case 'enumerated':
				this.progress.filesSeen = event.files;
				this.progress.bytesSeen = event.bytes;
				break;

			case 'progress':
				this.progress.phase = event.phase;
				this.progress.done = event.done;
				this.progress.total = event.total;
				this.progress.bytesDone = event.bytesDone;
				this.progress.bytesTotal = event.bytesTotal;
				break;

			case 'foundSoFar':
				this.progress.foundGroups = event.duplicateGroups;
				this.progress.foundBytes = event.wastedBytes;
				break;

			case 'examining':
				this.progress.phase = event.phase;
				this.progress.examining = event.path;
				this.progress.examined = event.count;
				break;

			case 'skipped':
				if (this.skipped.length < MAX_SKIPPED) {
					this.skipped.push({ path: event.path, reason: event.reason });
				}
				break;

			case 'finished':
				this.summary = event.summary;
				this.activity = 'idle';
				this.progress.phase = 'done';
				void this.loadResults();
				break;
		}
	}

	async startScan() {
		this.error = null;
		this.notice = null;
		this.skipped = [];
		this.summary = null;
		this.selected = {};
		this.duplicates = [];
		this.folders = [];
		this.similar = [];
		this.duplicateTotal = 0;
		this.folderTotal = 0;
		this.similarTotal = 0;
		this.progress = emptyProgress();

		try {
			await api.startScan($state.snapshot(this.scanConfig));
			this.activity = 'scanning';
			this.tab = 'duplicates';
		} catch (error) {
			this.error = String(error);
		}
	}

	async startOrphanScan() {
		this.error = null;
		this.notice = null;
		this.orphans = [];
		this.orphanTotal = 0;
		this.orphanUnavailable = [];
		this.progress = emptyProgress();

		try {
			await api.startOrphanScan($state.snapshot(this.orphanConfig));
			this.activity = 'findingOrphans';
			this.tab = 'orphans';
		} catch (error) {
			this.error = String(error);
		}
	}

	async cancel() {
		await api.cancelScan();
		this.activity = 'idle';
	}

	/**
	 * Load the duplicates that are already final, without touching the photo
	 * results the scan is still working on.
	 */
	async loadDuplicatesEarly() {
		try {
			const page = await api.duplicatePage(0, PAGE_SIZE);
			this.duplicates = page.groups;
			this.duplicateTotal = page.total;
			this.summary = page.summary;
		} catch {
			// The scan may have been cancelled between the event and this call.
		}
	}

	async loadResults() {
		try {
			const duplicates = await api.duplicatePage(0, PAGE_SIZE);
			this.duplicates = duplicates.groups;
			this.duplicateTotal = duplicates.total;
			this.summary = duplicates.summary;

			const folders = await api.folderPage(0, PAGE_SIZE);
			this.folders = folders.groups;
			this.folderTotal = folders.total;
			this.folderReclaimable = folders.reclaimableBytes;

			const similar = await api.similarPage(0, PAGE_SIZE);
			this.similar = similar.groups;
			this.similarTotal = similar.total;
		} catch (error) {
			// "no scan has been run yet" is expected right after a cancel.
			if (!String(error).includes('no scan')) this.error = String(error);
		}
	}

	async loadOrphans() {
		try {
			const page = await api.orphanPage(0, PAGE_SIZE);
			this.orphans = page.findings;
			this.orphanTotal = page.total;
			this.orphanReclaimable = page.reclaimableBytes;
			this.orphanUnavailable = page.unavailable;
		} catch (error) {
			if (!String(error).includes('no orphan')) this.error = String(error);
		}
	}

	async loadMoreDuplicates() {
		if (this.loadingMore || this.duplicates.length >= this.duplicateTotal) return;

		this.loadingMore = true;
		try {
			const page = await api.duplicatePage(this.duplicates.length, PAGE_SIZE);
			this.duplicates = [...this.duplicates, ...page.groups];
		} finally {
			this.loadingMore = false;
		}
	}

	async loadMoreFolders() {
		if (this.loadingMore || this.folders.length >= this.folderTotal) return;

		this.loadingMore = true;
		try {
			const page = await api.folderPage(this.folders.length, PAGE_SIZE);
			this.folders = [...this.folders, ...page.groups];
		} finally {
			this.loadingMore = false;
		}
	}

	/**
	 * Apply a keep rule across every duplicated folder.
	 *
	 * The suggestions come back as per-file plans, so the selection this
	 * produces is identical in kind to one made on the duplicate list — the
	 * action bar needs to know nothing about folders.
	 */
	async autoSelectFolders(rule: Parameters<typeof api.suggestFolderPlan>[0]) {
		const plans = await api.suggestFolderPlan(rule);
		this.applySuggestion(plans);
	}

	/**
	 * Select everything in one folder group except the named copy.
	 *
	 * The engine resolves this, not the front end: the files involved often sit
	 * outside the page the UI has loaded, and the engine always has all of them.
	 */
	async keepFolder(groupId: number, keep: string) {
		const plans = await api.folderPlan(groupId, keep);
		this.applySuggestion(plans);
	}

	async loadMoreSimilar() {
		if (this.loadingMore || this.similar.length >= this.similarTotal) return;

		this.loadingMore = true;
		try {
			const page = await api.similarPage(this.similar.length, PAGE_SIZE);
			this.similar = [...this.similar, ...page.groups];
		} finally {
			this.loadingMore = false;
		}
	}

	async loadMoreOrphans() {
		if (this.loadingMore || this.orphans.length >= this.orphanTotal) return;

		this.loadingMore = true;
		try {
			const page = await api.orphanPage(this.orphans.length, PAGE_SIZE);
			this.orphans = [...this.orphans, ...page.findings];
		} finally {
			this.loadingMore = false;
		}
	}

	isSelected(path: string): boolean {
		return this.selected[path] === true;
	}

	/**
	 * Toggle one path, refusing to select the last remaining member of a group.
	 *
	 * `siblings` is every path in the same group, so the rule can be checked
	 * here rather than being discovered when the plan is built.
	 */
	toggle(path: string, siblings: string[]) {
		if (this.selected[path]) {
			delete this.selected[path];
			this.notice = null;
			return;
		}

		const remaining = siblings.filter((p) => p !== path && !this.selected[p]);
		if (remaining.length === 0) {
			this.notice = 'Every group has to keep one copy. Unselect another file first.';
			return;
		}

		this.selected[path] = true;
		this.notice = null;
	}

	/** Select every member of a group except `keep`. */
	selectAllBut(keep: string, siblings: string[]) {
		for (const path of siblings) {
			if (path === keep) delete this.selected[path];
			else this.selected[path] = true;
		}
		this.notice = null;
	}

	clearSelection() {
		this.selected = {};
		this.notice = null;
	}

	/** Orphans have no keeper, so any subset is selectable. */
	toggleOrphan(path: string) {
		if (this.selected[path]) delete this.selected[path];
		else this.selected[path] = true;
	}

	/** Apply a keeper rule to every duplicate group at once. */
	async autoSelectDuplicates(rule: Parameters<typeof api.suggestDuplicatePlan>[0]) {
		const plans = await api.suggestDuplicatePlan(rule);
		this.applySuggestion(plans);
	}

	async autoSelectSimilar(rule: Parameters<typeof api.suggestSimilarPlan>[0]) {
		const plans = await api.suggestSimilarPlan(rule);
		this.applySuggestion(plans);
	}

	private applySuggestion(plans: { keep: string; remove: string[] }[]) {
		for (const plan of plans) {
			delete this.selected[plan.keep];
			for (const path of plan.remove) this.selected[path] = true;
		}
		this.notice = `Selected ${plans.reduce((n, p) => n + p.remove.length, 0)} files.`;
	}

	/**
	 * Turn the current selection into group plans.
	 *
	 * Groups with nothing selected are skipped; groups with everything
	 * selected cannot occur, because {@link toggle} prevents them.
	 */
	buildGroupPlans(): { groupId: number; keep: string; remove: string[] }[] {
		const plans: { groupId: number; keep: string; remove: string[] }[] = [];

		const collect = (id: number, files: FileEntry[]) => {
			const remove = files.filter((f) => this.selected[f.path]).map((f) => f.path);
			if (remove.length === 0) return;

			const keep = files.find((f) => !this.selected[f.path]);
			if (!keep) return;

			plans.push({ groupId: id, keep: keep.path, remove });
		};

		for (const group of this.duplicates) collect(group.id, group.files);
		for (const group of this.similar) collect(group.id, group.images.map((i) => i.file));

		return plans;
	}

	/**
	 * Whether the selection contains anything a restore point would cover.
	 *
	 * Broken shortcuts and empty folders are not program state, so offering a
	 * restore point for a selection of only those would be ceremony — and
	 * ceremony around safety teaches people to click past it.
	 */
	selectionTouchesPrograms(): boolean {
		return this.orphans.some(
			(finding) =>
				this.selected[finding.path] &&
				(finding.kind === 'leftoverAppData' ||
					finding.kind === 'orphanedInstallerPackage')
		);
	}

	selectedOrphanPaths(): string[] {
		return this.orphans.filter((f) => this.selected[f.path]).map((f) => f.path);
	}
}

export const store = new Store();
