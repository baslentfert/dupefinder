/**
 * Mirrors the serde representation of the Rust types in `dupe-core`.
 *
 * Every Rust type there uses `rename_all = "camelCase"`, and the tagged enums
 * use `kind` (scan events, orphan kinds) or `type` (actions), so the shapes
 * below are the wire format verbatim. If a field name here is wrong the
 * failure is silent — a value simply reads as `undefined` — so keep this in
 * step with `crates/dupe-core/src/model.rs` and `config.rs`.
 */

export type ScanStrategy = 'walk' | 'mft' | 'auto';

export interface Detectors {
	exact: boolean;
	similarImages: boolean;
}

export interface ScanConfig {
	roots: string[];
	strategy: ScanStrategy;
	detectors: Detectors;
	minSize: number;
	maxSize: number | null;
	excludePatterns: string[];
	onlyExtensions: string[] | null;
	includeHidden: boolean;
	followSymlinks: boolean;
	skipSystemPaths: boolean;
	verifyBytes: boolean;
	/** Leave OneDrive/Dropbox files that are only stored in the cloud alone. */
	skipCloudPlaceholders: boolean;
	/** Hamming distance in bits, out of 64. */
	similarityThreshold: number;
	/** Images shorter than this on their short side are not compared. */
	minImageSide: number;
	threads: number | null;
}

export interface FileEntry {
	path: string;
	size: number;
	/** Seconds since the unix epoch. */
	mtime: number;
}

export interface DuplicateGroup {
	id: number;
	/** BLAKE3 digest, hex. */
	hash: string;
	/** Size of one member, in bytes. */
	size: number;
	files: FileEntry[];
}

export interface FolderCopy {
	path: string;
	totalFiles: number;
	totalBytes: number;
	/** Files whose content the other folders in the group also hold. */
	sharedFiles: number;
	/** Files that exist nowhere else — removing this folder would lose them. */
	uniqueFiles: number;
}

export interface FolderGroup {
	id: number;
	folders: FolderCopy[];
	/** Distinct files every folder in the group holds a copy of. */
	sharedFiles: number;
	/** Size of one copy of that shared content. */
	sharedBytes: number;
	/** No folder holds anything the others do not, so any copy may be kept. */
	interchangeable: boolean;
}

export interface FolderPage {
	groups: FolderGroup[];
	total: number;
	reclaimableBytes: number;
}

export interface SimilarImage {
	file: FileEntry;
	width: number;
	height: number;
	/** Bits of difference from the cluster's reference image. */
	distance: number;
	takenAt: number | null;
}

export interface SimilarGroup {
	id: number;
	images: SimilarImage[];
}

export interface Summary {
	filesSeen: number;
	bytesSeen: number;
	filesHashed: number;
	bytesHashed: number;
	duplicateGroups: number;
	duplicateFiles: number;
	wastedBytes: number;
	similarGroups: number;
	similarImages: number;
	skipped: number;
	elapsedMs: number;
	cacheHits: number;
}

export type Phase =
	| 'enumerating'
	| 'groupingBySize'
	| 'hashingPartial'
	| 'hashingFull'
	| 'verifyingBytes'
	| 'hashingImages'
	| 'clusteringImages'
	| 'scanningShortcuts'
	| 'scanningEmptyFolders'
	| 'scanningAppData'
	| 'scanningInstallerCache'
	| 'done';

export type ScanEvent =
	| { kind: 'phaseStarted'; phase: Phase }
	| { kind: 'enumerated'; files: number; bytes: number }
	| {
			kind: 'progress';
			phase: Phase;
			done: number;
			total: number;
			bytesDone: number;
			bytesTotal: number;
	  }
	| { kind: 'examining'; phase: Phase; path: string; count: number }
	| {
			kind: 'foundSoFar';
			duplicateGroups: number;
			duplicateFiles: number;
			wastedBytes: number;
	  }
	| { kind: 'skipped'; path: string; reason: string }
	| { kind: 'finished'; summary: Summary };

export type OrphanKind =
	| 'brokenShortcut'
	| 'emptyFolder'
	| 'leftoverAppData'
	| 'orphanedInstallerPackage';

export type Confidence = 'suspected' | 'likely' | 'certain';

export interface OrphanFinding {
	id: number;
	kind: OrphanKind;
	path: string;
	sizeBytes: number;
	isDirectory: boolean;
	confidence: Confidence;
	reason: string;
	lastModified: number | null;
}

export interface DetectorUnavailable {
	kind: OrphanKind;
	reason: string;
}

export interface OrphanConfig {
	roots: string[];
	brokenShortcuts: boolean;
	emptyFolders: boolean;
	leftoverAppdata: boolean;
	orphanedInstaller: boolean;
	appdataUntouchedDays: number;
	ignoreShellMetadata: boolean;
}

export type Action =
	| { type: 'recycle' }
	| { type: 'deletePermanently' }
	| { type: 'moveTo'; destination: string }
	| { type: 'replaceWithHardLink' };

export interface GroupPlan {
	groupId: number;
	keep: string;
	remove: string[];
}

export interface Plan {
	action: Action;
	groups: GroupPlan[];
}

export interface OrphanPlan {
	action: Action;
	paths: string[];
	/**
	 * Ask Windows for a restore point first.
	 *
	 * Only offered for orphans: a restore point covers programs and the
	 * registry, not user data, so it would be false comfort on the duplicate
	 * and photo tabs.
	 */
	createRestorePoint: boolean;
}

export type RestorePointAvailability =
	| 'ready'
	| 'needsElevation'
	| 'protectionDisabled'
	| 'unsupported';

export interface Violation {
	groupId: number;
	path: string | null;
	reason: string;
}

export interface Outcome {
	path: string;
	ok: boolean;
	error: string | null;
	bytesFreed: number;
}

export interface ActionReport {
	outcomes: Outcome[];
	succeeded: number;
	failed: number;
	bytesFreed: number;
}

export type KeepRule = 'oldest' | 'newest' | 'shortestPath' | 'highestResolution';

export interface VolumeInfo {
	letter: string;
	label: string;
	fileSystem: string;
	totalBytes: number;
	freeBytes: number;
	isFixed: boolean;
	isNtfs: boolean;
}

export interface SuggestedRoot {
	label: string;
	path: string;
}

export interface SystemInfo {
	elevated: boolean;
	mftAvailable: boolean;
	logicalCores: number;
	volumes: VolumeInfo[];
	suggestedRoots: SuggestedRoot[];
}

export interface DuplicatePage {
	groups: DuplicateGroup[];
	total: number;
	summary: Summary;
}

export interface SimilarPage {
	groups: SimilarGroup[];
	total: number;
}

export interface OrphanPage {
	findings: OrphanFinding[];
	total: number;
	reclaimableBytes: number;
	unavailable: DetectorUnavailable[];
}

export type Activity = 'idle' | 'scanning' | 'findingOrphans';

export function defaultScanConfig(): ScanConfig {
	return {
		roots: [],
		strategy: 'auto',
		detectors: { exact: true, similarImages: true },
		// Empty files are all identical to each other and never interesting.
		minSize: 1,
		maxSize: null,
		excludePatterns: [],
		onlyExtensions: null,
		includeHidden: true,
		followSymlinks: false,
		skipSystemPaths: true,
		verifyBytes: false,
		skipCloudPlaceholders: true,
		similarityThreshold: 10,
		minImageSide: 64,
		threads: null
	};
}

export function defaultOrphanConfig(): OrphanConfig {
	return {
		roots: [],
		brokenShortcuts: true,
		emptyFolders: true,
		leftoverAppdata: true,
		orphanedInstaller: true,
		appdataUntouchedDays: 180,
		ignoreShellMetadata: true
	};
}
