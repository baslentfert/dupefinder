/** Presentation helpers. */

const UNITS = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'];

/** `1536` becomes `1.5 KB`. Byte counts stay whole. */
export function bytes(value: number): string {
	if (!Number.isFinite(value) || value < 0) return '—';
	if (value < 1024) return `${value} B`;

	let scaled = value;
	let unit = 0;
	while (scaled >= 1024 && unit < UNITS.length - 1) {
		scaled /= 1024;
		unit += 1;
	}
	return `${scaled.toFixed(scaled < 10 ? 1 : 0)} ${UNITS[unit]}`;
}

export function count(value: number): string {
	return value.toLocaleString('nl-NL');
}

export function duration(ms: number): string {
	if (ms < 1000) return `${ms} ms`;
	const seconds = ms / 1000;
	if (seconds < 60) return `${seconds.toFixed(1)} s`;

	const minutes = Math.floor(seconds / 60);
	const rest = Math.round(seconds % 60);
	return `${minutes}m ${rest}s`;
}

/** Unix seconds to a short local date. */
export function date(seconds: number | null): string {
	if (seconds === null || seconds <= 0) return '—';
	return new Date(seconds * 1000).toLocaleDateString('nl-NL', {
		year: 'numeric',
		month: 'short',
		day: 'numeric'
	});
}

export function fileName(path: string): string {
	const parts = path.split(/[\\/]/);
	return parts[parts.length - 1] || path;
}

/**
 * The name of the folder a file sits in.
 *
 * This is what distinguishes copies of a duplicate: their file names are
 * identical by definition, so labelling a button with one says nothing.
 */
export function parentName(path: string): string {
	const parts = path.split(/[\\/]/).filter(Boolean);
	return parts.length >= 2 ? parts[parts.length - 2] : path;
}

/** The folder's own name, i.e. the last segment of its path. */
export function folderName(path: string): string {
	const parts = path.split(/[\\/]/).filter(Boolean);
	return parts[parts.length - 1] ?? path;
}

export function directory(path: string): string {
	const index = path.search(/[\\/][^\\/]*$/);
	return index > 0 ? path.slice(0, index) : path;
}

/**
 * Shorten a long path from the middle, keeping the drive and the file name —
 * the two parts a user actually navigates by.
 */
export function shortenPath(path: string, max = 64): string {
	if (path.length <= max) return path;

	const name = fileName(path);
	const head = path.slice(0, Math.max(0, max - name.length - 4));
	return `${head}…\\${name}`;
}

/** A 64-bit Hamming distance as a rough "how alike" percentage. */
export function similarity(distance: number): string {
	const percent = Math.max(0, Math.round((1 - distance / 64) * 100));
	return `${percent}%`;
}

export function phaseLabel(phase: string): string {
	const labels: Record<string, string> = {
		enumerating: 'Listing files',
		groupingBySize: 'Grouping by size',
		hashingPartial: 'Comparing file heads and tails',
		hashingFull: 'Hashing candidates',
		verifyingBytes: 'Verifying byte for byte',
		hashingImages: 'Reading image previews',
		clusteringImages: 'Grouping similar photos',
		scanningShortcuts: 'Checking shortcuts',
		scanningEmptyFolders: 'Looking for empty folders',
		scanningAppData: 'Checking application data',
		scanningInstallerCache: 'Checking the installer cache',
		done: 'Finished'
	};
	return labels[phase] ?? phase;
}

export function orphanKindLabel(kind: string): string {
	const labels: Record<string, string> = {
		brokenShortcut: 'Broken shortcut',
		emptyFolder: 'Empty folder',
		leftoverAppData: 'Leftover app data',
		orphanedInstallerPackage: 'Orphaned installer package'
	};
	return labels[kind] ?? kind;
}

export function confidenceLabel(confidence: string): string {
	const labels: Record<string, string> = {
		certain: 'Verified',
		likely: 'Likely',
		suspected: 'Suspected'
	};
	return labels[confidence] ?? confidence;
}
