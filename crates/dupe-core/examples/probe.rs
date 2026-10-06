//! Quick real-world check of the engine, outside the app.
//!
//! ```text
//! cargo run --release -p dupe-core --example probe -- scan  C:\Users\me\Pictures
//! cargo run --release -p dupe-core --example probe -- orphans C:\Users\me\Desktop
//! ```

use std::path::PathBuf;
use std::time::Instant;

use rayon::prelude::*;

use dupe_core::orphans::{OrphanConfig, OrphanKind};
use dupe_core::{Detectors, Reporter, ScanConfig, ScanStrategy};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("dupe_core=info")
        .init();

    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_else(|| "scan".into());
    let roots: Vec<PathBuf> = args.map(PathBuf::from).collect();

    if roots.is_empty() && mode != "drives" {
        eprintln!("usage: probe <scan|orphans> <path> [path...]");
        eprintln!("       probe drives");
        std::process::exit(2);
    }

    match mode.as_str() {
        "scan" => scan(roots),
        "orphans" => orphans(roots),
        "drives" => drives(),
        "preview" => preview(roots),
        "report" => report(roots),
        "prefilter" => prefilter(roots),
        other => eprintln!("unknown mode {other}"),
    }
}

/// Scans, then writes the HTML and CSV reports next to each other, so the
/// rendering can be checked against real findings rather than fixtures.
fn report(roots: Vec<PathBuf>) {
    let cfg = ScanConfig {
        roots: roots.clone(),
        strategy: ScanStrategy::Walk,
        ..Default::default()
    };

    let cache = std::env::var("LOCALAPPDATA")
        .ok()
        .map(|dir| PathBuf::from(dir).join("com.baslentfert.dupefinder/hashes.redb"));

    let result = dupe_core::scan(&cfg, &Reporter::silent(), cache).expect("scan failed");

    let orphans = dupe_core::orphans::find(
        &OrphanConfig {
            roots: roots.clone(),
            ..Default::default()
        },
        &Reporter::silent(),
    )
    .map(|r| r.findings)
    .unwrap_or_default();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let input = dupe_core::report::ReportInput {
        generated_at: now,
        roots: &roots,
        summary: &result.summary,
        duplicates: &result.duplicates,
        similar: &result.similar,
        orphans: &orphans,
        options: dupe_core::report::ReportOptions::default(),
    };

    let started = Instant::now();
    let html = dupe_core::report::to_html(&input);
    println!(
        "html: {} in {:.2}s",
        human(html.len() as u64),
        started.elapsed().as_secs_f64()
    );
    std::fs::write("report.html", &html).expect("could not write report.html");

    let csv = dupe_core::report::to_csv(&input, dupe_core::platform::list_separator());
    println!(
        "csv:  {} ({} rows, separator {:?})",
        human(csv.len() as u64),
        csv.lines().count() - 1,
        dupe_core::platform::list_separator()
    );
    std::fs::write("report.csv", &csv).expect("could not write report.csv");
}

/// Exercises exactly what the thumbnail protocol serves, so a blank preview in
/// the UI can be pinned on the decoder or on the protocol, not guessed at.
fn preview(paths: Vec<PathBuf>) {
    for path in paths {
        let started = Instant::now();
        match dupe_core::images::decode::load_preview(&path, 224) {
            Ok(preview) => println!(
                "  ok   {}x{} ({} bytes) in {:.0}ms  {}",
                preview.width,
                preview.height,
                preview.rgb.len(),
                started.elapsed().as_secs_f64() * 1000.0,
                path.display()
            ),
            Err(err) => println!("  FAIL {}: {err}", path.display()),
        }

        match dupe_core::images::decode::load(&path) {
            Ok(thumb) => println!(
                "       hash thumbnail ok, source {}x{}",
                thumb.width, thumb.height
            ),
            Err(err) => println!("       hash thumbnail FAILED: {err}"),
        }
    }
}

/// Times drive enumeration, which happens before the UI can render anything.
fn drives() {
    let started = Instant::now();
    let volumes = dupe_core::platform::volumes();
    println!("volumes() took {:.3}s", started.elapsed().as_secs_f64());

    for volume in &volumes {
        println!(
            "  {}: {:20} {:6} {:>10} free of {:>10}{}",
            volume.letter,
            volume.label,
            volume.file_system,
            human(volume.free_bytes),
            human(volume.total_bytes),
            if volume.is_fixed {
                ""
            } else {
                "  (removable/remote)"
            }
        );
    }

    let started = Instant::now();
    let elevated = dupe_core::platform::is_elevated();
    println!(
        "is_elevated() = {elevated}, took {:.3}s",
        started.elapsed().as_secs_f64()
    );
}

fn scan(roots: Vec<PathBuf>) {
    let cfg = ScanConfig {
        roots,
        strategy: ScanStrategy::Walk,
        detectors: Detectors {
            exact: true,
            similar_images: true,
        },
        ..Default::default()
    };

    let started = Instant::now();
    let result = dupe_core::scan(&cfg, &Reporter::silent(), None).expect("scan failed");
    let elapsed = started.elapsed();

    let s = &result.summary;
    println!(
        "{} files ({}) in {:.2}s",
        s.files_seen,
        human(s.bytes_seen),
        elapsed.as_secs_f64()
    );
    println!(
        "  hashed {} files ({}), {} cache hits, {} skipped",
        s.files_hashed,
        human(s.bytes_hashed),
        s.cache_hits,
        s.skipped
    );
    println!(
        "  {} duplicate groups, {} files, {} reclaimable",
        s.duplicate_groups,
        s.duplicate_files,
        human(s.wasted_bytes)
    );
    println!(
        "  {} similar-image clusters covering {} images",
        s.similar_groups, s.similar_images
    );

    let swappable = result.folders.iter().filter(|g| g.interchangeable).count();
    let reclaimable: u64 = result.folders.iter().map(|g| g.wasted_bytes()).sum();
    println!(
        "  {} duplicated-folder groups ({} fully interchangeable), {} reclaimable",
        result.folders.len(),
        swappable,
        human(reclaimable)
    );

    for group in result.folders.iter().take(4) {
        println!(
            "
  {} copies of a folder - {} shared files, {} each, {} reclaimable{}",
            group.folders.len(),
            group.shared_files,
            human(group.shared_bytes),
            human(group.wasted_bytes()),
            if group.interchangeable {
                ""
            } else {
                "  [has unique files]"
            }
        );
        for folder in group.folders.iter().take(3) {
            println!(
                "    {:<4} unique  {}",
                folder.unique_files,
                folder.path.display()
            );
        }
    }

    for group in result.duplicates.iter().take(5) {
        println!("\n  {} x {}", group.files.len(), human(group.size));
        for file in &group.files {
            println!("    {}", file.path.display());
        }
    }

    for group in result.similar.iter().take(5) {
        println!("\n  similar cluster of {}", group.images.len());
        for image in &group.images {
            println!(
                "    d={:2} {}x{} {}",
                image.distance,
                image.width,
                image.height,
                image.file.path.display()
            );
        }
    }
}

fn orphans(roots: Vec<PathBuf>) {
    let cfg = OrphanConfig {
        roots,
        ..Default::default()
    };

    let started = Instant::now();
    let report = dupe_core::orphans::find(&cfg, &Reporter::silent()).expect("orphan scan failed");

    println!(
        "{} findings, {} reclaimable, in {:.2}s",
        report.findings.len(),
        human(report.reclaimable_bytes),
        started.elapsed().as_secs_f64()
    );

    for unavailable in &report.unavailable {
        println!(
            "  [unavailable] {:?}: {}",
            unavailable.kind, unavailable.reason
        );
    }

    for kind in [
        OrphanKind::BrokenShortcut,
        OrphanKind::EmptyFolder,
        OrphanKind::OrphanedInstallerPackage,
        OrphanKind::LeftoverAppData,
    ] {
        let matching: Vec<_> = report.findings.iter().filter(|f| f.kind == kind).collect();
        if matching.is_empty() {
            continue;
        }
        println!("\n  {:?}: {}", kind, matching.len());
        for finding in matching.iter().take(8) {
            println!(
                "    [{:?}] {} ({}) - {}",
                finding.confidence,
                finding.path.display(),
                human(finding.size_bytes),
                finding.reason
            );
        }
    }
}

fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Measures what each pre-filter strategy would actually cost on real files.
///
/// The pre-filter exists to avoid reading a file in full. A wider sample
/// eliminates more candidates — but it is paid on *every* candidate, including
/// the ones a narrow sample would already have ruled out. Whether that trade
/// pays is a property of the data, so this reruns the stage for each strategy
/// and reports total bytes read, with the full hashes as ground truth.
fn prefilter(roots: Vec<PathBuf>) {
    use std::collections::{HashMap, HashSet};

    use dupe_core::hash::{full_hash, partial_hash_with, Sampling, DEFAULT_SAMPLING};

    let cfg = ScanConfig {
        roots,
        strategy: ScanStrategy::Walk,
        ..Default::default()
    };

    let entries = dupe_core::scan::enumerate(&cfg, &Reporter::silent()).expect("walk failed");

    // Stage 1 — size, exactly as the engine does it.
    let mut by_size: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        by_size.entry(e.size).or_default().push(i);
    }
    let candidates: Vec<usize> = by_size
        .into_values()
        .filter(|g| g.len() > 1)
        .flatten()
        .collect();

    let candidate_bytes: u64 = candidates.iter().map(|&i| entries[i].size).sum();
    println!(
        "{} files walked; {} survive size grouping ({})",
        entries.len(),
        candidates.len(),
        human(candidate_bytes)
    );

    // Ground truth: which candidates really are duplicates. Anything a filter
    // passes beyond these is a full read that bought nothing.
    let hashed: Vec<(usize, dupe_core::ContentHash)> = candidates
        .par_iter()
        .filter_map(|&i| {
            full_hash(&entries[i].path, entries[i].size)
                .ok()
                .map(|h| (i, h))
        })
        .collect();

    let mut by_hash: HashMap<dupe_core::ContentHash, Vec<usize>> = HashMap::new();
    for (i, h) in &hashed {
        by_hash.entry(*h).or_default().push(*i);
    }
    let real: HashSet<usize> = by_hash
        .values()
        .filter(|g| g.len() > 1)
        .flatten()
        .copied()
        .collect();

    let floor: u64 = real.iter().map(|&i| entries[i].size).sum();
    println!(
        "{} are genuine duplicates ({}) — the floor any filter must still read\n",
        real.len(),
        human(floor)
    );

    let strategies = [
        ("4K edges (current)", DEFAULT_SAMPLING),
        (
            "4K edges + middle",
            Sampling {
                window: 4096,
                middle: true,
            },
        ),
        (
            "16K edges",
            Sampling {
                window: 16384,
                middle: false,
            },
        ),
        (
            "16K edges + middle",
            Sampling {
                window: 16384,
                middle: true,
            },
        ),
        (
            "64K edges",
            Sampling {
                window: 65536,
                middle: false,
            },
        ),
        (
            "64K edges + middle",
            Sampling {
                window: 65536,
                middle: true,
            },
        ),
    ];

    println!(
        "{:<21} {:>9} {:>7} {:>11} {:>11} {:>11} {:>8}",
        "strategy", "survive", "wasted", "sample", "full read", "total", "vs now"
    );

    let mut baseline = 0u64;

    for (label, sampling) in strategies {
        let partials: Vec<(usize, u64, bool)> = candidates
            .par_iter()
            .filter_map(|&i| {
                let e = &entries[i];
                partial_hash_with(&e.path, e.size, sampling)
                    .ok()
                    .map(|p| (i, p.value, p.complete))
            })
            .collect();

        // A file small enough to be covered entirely by the sample is read
        // whole here, and never reaches the full stage.
        let sample_cost: u64 = partials
            .iter()
            .map(|&(i, _, complete)| {
                let size = entries[i].size;
                if complete {
                    size
                } else {
                    sampling.cost().min(size)
                }
            })
            .sum();

        let mut groups: HashMap<u64, Vec<usize>> = HashMap::new();
        for &(i, value, complete) in &partials {
            if !complete {
                groups.entry(value).or_default().push(i);
            }
        }

        let survivors: Vec<usize> = groups
            .into_values()
            .filter(|g| g.len() > 1)
            .flatten()
            .collect();

        let full_cost: u64 = survivors.iter().map(|&i| entries[i].size).sum();
        let wasted = survivors.iter().filter(|i| !real.contains(i)).count();
        let total = sample_cost + full_cost;
        if baseline == 0 {
            baseline = total;
        }

        let delta = if total <= baseline {
            format!(
                "-{:.1}%",
                (baseline - total) as f64 / baseline as f64 * 100.0
            )
        } else {
            format!(
                "+{:.1}%",
                (total - baseline) as f64 / baseline as f64 * 100.0
            )
        };

        println!(
            "{:<21} {:>9} {:>7} {:>11} {:>11} {:>11} {:>8}",
            label,
            survivors.len(),
            wasted,
            human(sample_cost),
            human(full_cost),
            human(total),
            delta
        );
    }
}
