use crate::model::*;
use crate::scanner::classify::*;
use crate::util::{is_media_ext, is_vm_ext};
use bytesize::ByteSize;
use crossbeam_channel::Sender;
use jwalk::WalkDir;
use rayon::prelude::*;
use std::fs;
use std::os::unix::fs::MetadataExt;
#[cfg(target_os = "macos")]
use std::os::macos::fs::MetadataExt as DarwinMetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

const SF_DATALESS: u32 = 0x40000000;

/// Fast git dirty check using only filesystem metadata (no subprocess).
/// Compares .git/index mtime against working tree file mtimes.
/// If any tracked file area has been modified after the index, it's "dirty."
fn check_git_dirty_fast(path: &Path) -> String {
    let git_dir = path.join(".git");
    if !git_dir.is_dir() {
        return "Not a git repo".to_string();
    }

    // Get .git/index mtime — this is when git last staged changes
    let index_path = git_dir.join("index");
    let index_mtime = match fs::metadata(&index_path).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(_) => return "Git repo (no index)".to_string(),
    };

    // Check for in-progress operations
    if git_dir.join("MERGE_HEAD").exists() {
        return "!! Merge in progress".to_string();
    }
    if git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists() {
        return "!! Rebase in progress".to_string();
    }

    // Quick scan: check if any file in the top 2 levels of the worktree
    // was modified after the index. This catches most dirty states fast.
    let has_newer = has_files_newer_than(path, index_mtime, 2, &git_dir);

    if has_newer {
        "May have uncommitted changes".to_string()
    } else {
        "Clean — safe to remove".to_string()
    }
}

fn has_files_newer_than(dir: &Path, cutoff: SystemTime, depth: u32, skip: &Path) -> bool {
    if depth == 0 { return false; }
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return false,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path == *skip { continue; }
        if let Ok(meta) = entry.metadata() {
            if meta.is_file() {
                if let Ok(mtime) = meta.modified() {
                    if mtime > cutoff { return true; }
                }
            } else if meta.is_dir() {
                let name = entry.file_name();
                let n = name.to_string_lossy();
                // Skip common non-source dirs
                if n == "node_modules" || n == "target" || n == ".build" || n == "build"
                    || n == "__pycache__" || n == ".venv" || n == "venv"
                {
                    continue;
                }
                if has_files_newer_than(&path, cutoff, depth - 1, skip) {
                    return true;
                }
            }
        }
    }
    false
}

pub fn get_user_homes() -> Vec<PathBuf> {
    let mut homes = Vec::new();
    if let Ok(entries) = fs::read_dir("/Users") {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                if name != "Shared" && !name.starts_with('.') {
                    homes.push(path);
                }
            }
        }
    }
    homes
}

fn dir_physical_size(path: &Path) -> (u64, u64, Option<SystemTime>) {
    let mut physical: u64 = 0;
    let mut logical: u64 = 0;
    let mut newest: Option<SystemTime> = None;

    fn walk(path: &Path, physical: &mut u64, logical: &mut u64, newest: &mut Option<SystemTime>) {
        let entries = match fs::read_dir(path) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_dir() {
                walk(&entry.path(), physical, logical, newest);
            } else {
                let flags = meta.st_flags() as u32;
                if flags & SF_DATALESS != 0 { continue; }
                *physical += (meta.blocks() as u64) * 512;
                *logical += meta.len();
                if let Ok(modified) = meta.modified() {
                    *newest = Some(newest.map_or(modified, |n: SystemTime| n.max(modified)));
                }
            }
        }
    }
    walk(path, &mut physical, &mut logical, &mut newest);
    (physical, logical, newest)
}

struct Bucket {
    category: Category,
    physical: u64,
    logical: u64,
    newest: Option<SystemTime>,
    uid: u32,
    cloud_backed: bool,
    detail: String,
}

pub fn run_scan(tx: Sender<ScanEvent>) {
    let start = Instant::now();
    let files_scanned = AtomicU64::new(0);
    let perm_errors = AtomicU64::new(0);
    let dataless_skipped = AtomicU64::new(0);
    let mut last_progress = Instant::now();

    let send_progress = |phase: ScanPhase, files: &AtomicU64, errs: &AtomicU64, dl: &AtomicU64, start: Instant, tx: &Sender<ScanEvent>| {
        let _ = tx.send(ScanEvent::Progress(ScanProgress {
            phase,
            files_scanned: files.load(Ordering::Relaxed),
            perm_errors: errs.load(Ordering::Relaxed),
            dataless_skipped: dl.load(Ordering::Relaxed),
            elapsed: start.elapsed(),
        }));
    };

    // Phase 0: detect apps
    send_progress(ScanPhase::DetectingApps, &files_scanned, &perm_errors, &dataless_skipped, start, &tx);
    let installed_ids = get_installed_app_bundle_ids();

    // Phase 1: SINGLE-PASS WALK — classify directories as buckets, accumulate sizes inline
    let homes = get_user_homes();
    let mut buckets: Vec<(PathBuf, Bucket)> = Vec::new();

    // Downloads need special handling (age filter on top-level children)
    let cutoff_90d = SystemTime::now() - Duration::from_secs(90 * 24 * 3600);
    // Stale project needs special handling
    let cutoff_6mo = SystemTime::now() - Duration::from_secs(180 * 24 * 3600);

    for home in &homes {
        let user = home.file_name().unwrap_or_default().to_string_lossy().to_string();
        send_progress(ScanPhase::ScanningUser(user.clone()), &files_scanned, &perm_errors, &dataless_skipped, start, &tx);

        let walker = WalkDir::new(home)
            .skip_hidden(false)
            .follow_links(false)
            .sort(false)
            .parallelism(jwalk::Parallelism::RayonNewPool(num_cpus::get().min(8)));

        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => { perm_errors.fetch_add(1, Ordering::Relaxed); continue; }
            };

            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry.file_type().is_dir();
            let depth = entry.depth();

            files_scanned.fetch_add(1, Ordering::Relaxed);

            if last_progress.elapsed() > Duration::from_millis(150) {
                send_progress(ScanPhase::ScanningUser(user.clone()), &files_scanned, &perm_errors, &dataless_skipped, start, &tx);
                last_progress = Instant::now();
            }

            // Check if this entry is inside an existing bucket (reverse = most recent first)
            let in_bucket = buckets.iter().rposition(|(prefix, _)| path.starts_with(prefix));

            if is_dir && depth > 0 && in_bucket.is_none() {
                if let Some(cat) = classify_path(&path, &name, true, depth) {
                    // Downloads: skip entirely, we handle top-level children separately below
                    if cat == Category::OldDownloads {
                        // Scan top-level children by age — each old child becomes its own bucket
                        if let Ok(entries) = fs::read_dir(&path) {
                            for child in entries.flatten() {
                                let cpath = child.path();
                                if let Ok(meta) = cpath.symlink_metadata() {
                                    let old = meta.modified().ok().map_or(false, |t| t < cutoff_90d);
                                    if old && cpath.is_dir() {
                                        let uid = meta.uid();
                                        buckets.push((cpath, Bucket {
                                            category: Category::OldDownloads,
                                            physical: 0, logical: 0, newest: meta.modified().ok(),
                                            uid, cloud_backed: false,
                                            detail: child.file_name().to_string_lossy().to_string(),
                                        }));
                                    } else if old {
                                        let flags = meta.st_flags() as u32;
                                        if flags & SF_DATALESS != 0 { continue; }
                                        let phys = (meta.blocks() as u64) * 512;
                                        if phys > 500_000 {
                                            let cname = child.file_name().to_string_lossy().to_string();
                                            let _ = tx.send(ScanEvent::Found(Category::OldDownloads, Finding {
                                                path: cpath, physical_size: phys, logical_size: meta.len(),
                                                last_modified: meta.modified().ok(), owner_uid: meta.uid(),
                                                cloud_backed: false, detail: cname,
                                            }));
                                        }
                                    }
                                }
                            }
                        }
                        // Add Downloads itself as a "skip" bucket so jwalk entries inside get ignored
                        buckets.push((path.clone(), Bucket {
                            category: Category::OldDownloads,
                            physical: 0, logical: 0, newest: None, uid: 0,
                            cloud_backed: false, detail: "__skip__".to_string(),
                        }));
                        continue;
                    }

                    // Cloud-synced: only bucket top-level CloudStorage subdirs
                    if cat == Category::CloudSyncedLocal {
                        if depth <= 4 && path.to_string_lossy().contains("Library/CloudStorage/") && depth <= 3 {
                            let uid = fs::symlink_metadata(&path).map(|m| m.uid()).unwrap_or(0);
                            buckets.push((path.clone(), Bucket {
                                category: cat, physical: 0, logical: 0, newest: None,
                                uid, cloud_backed: true, detail: String::new(),
                            }));
                        }
                        continue;
                    }

                    // All other classified dirs — create bucket, accumulate from walk
                    let uid = fs::symlink_metadata(&path).map(|m| m.uid()).unwrap_or(0);
                    buckets.push((path.clone(), Bucket {
                        category: cat, physical: 0, logical: 0, newest: None,
                        uid, cloud_backed: false, detail: String::new(),
                    }));
                    continue;
                }

                // Stale dev projects
                if depth >= 2 && depth <= 5 && path.join(".git").is_dir() {
                    let uid = fs::symlink_metadata(&path).map(|m| m.uid()).unwrap_or(0);
                    buckets.push((path.clone(), Bucket {
                        category: Category::StaleProject, physical: 0, logical: 0,
                        newest: None, uid, cloud_backed: false, detail: String::new(),
                    }));
                    continue;
                }

                // Uninstalled app leftovers
                if path.to_string_lossy().contains("Library/Application Support/") && depth == 3 {
                    let bundle_like = name.replace(' ', ".").to_lowercase();
                    let is_installed = installed_ids.iter().any(|id| {
                        id.to_lowercase().contains(&bundle_like) || bundle_like.contains(&id.to_lowercase())
                    });
                    if !is_installed {
                        let uid = fs::symlink_metadata(&path).map(|m| m.uid()).unwrap_or(0);
                        buckets.push((path.clone(), Bucket {
                            category: Category::OldAppLeftovers, physical: 0, logical: 0,
                            newest: None, uid, cloud_backed: false,
                            detail: "App may no longer be installed".to_string(),
                        }));
                    }
                    continue;
                }

                // App caches
                if path.to_string_lossy().contains("Library/Caches/") && depth == 3 {
                    let uid = fs::symlink_metadata(&path).map(|m| m.uid()).unwrap_or(0);
                    buckets.push((path.clone(), Bucket {
                        category: Category::AppCache, physical: 0, logical: 0,
                        newest: None, uid, cloud_backed: false, detail: String::new(),
                    }));
                    continue;
                }
            }

            // Files: stat if inside a bucket OR if in Library/Application Support/CloudStorage
            // (where large files live). Skip stat for random home-dir files.
            if entry.file_type().is_file() {
                let path_str = path.to_string_lossy();
                let likely_interesting = in_bucket.is_some()
                    || path_str.contains("Library/")
                    || path_str.contains("Movies/")
                    || path_str.contains(".android/")
                    || path_str.contains(".cache/")
                    || path_str.contains(".codeium/");

                if !likely_interesting { continue; }

                if let Ok(meta) = path.symlink_metadata() {
                    let flags = meta.st_flags() as u32;
                    if flags & SF_DATALESS != 0 {
                        dataless_skipped.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    let phys = (meta.blocks() as u64) * 512;
                    let logical = meta.len();
                    let mtime = meta.modified().ok();

                    if let Some(idx) = in_bucket {
                        let bucket = &mut buckets[idx].1;
                        bucket.physical += phys;
                        bucket.logical += logical;
                        if let Some(mt) = mtime {
                            bucket.newest = Some(bucket.newest.map_or(mt, |n: SystemTime| n.max(mt)));
                        }
                    } else if phys > 200_000_000 {
                        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                        let cloud_backed = path.to_string_lossy().contains("CloudStorage")
                            || path.to_string_lossy().contains("Mobile Documents");
                        let cat = if is_vm_ext(&ext) { Category::VmImages }
                            else if is_media_ext(&ext) { Category::LargeMedia }
                            else { Category::LargeOther };
                        let _ = tx.send(ScanEvent::Found(cat, Finding {
                            path: path.clone(), physical_size: phys, logical_size: logical,
                            last_modified: mtime, owner_uid: meta.uid(),
                            cloud_backed, detail: ext,
                        }));
                    }
                }
            }
        }
    }

    // Emit findings from buckets
    for (path, bucket) in &buckets {
        if bucket.detail == "__skip__" { continue; } // Downloads parent placeholder

        match &bucket.category {
            Category::StaleProject => {
                let is_stale = bucket.newest.map_or(true, |t| t < cutoff_6mo);
                if !is_stale || bucket.physical < 10_000_000 { continue; }
                let detail = check_git_dirty_fast(path);
                let _ = tx.send(ScanEvent::Found(Category::StaleProject, Finding {
                    path: path.clone(), physical_size: bucket.physical, logical_size: bucket.logical,
                    last_modified: bucket.newest, owner_uid: bucket.uid, cloud_backed: false, detail,
                }));
            }
            Category::OldAppLeftovers => {
                if bucket.physical < 10_000_000 { continue; }
                let _ = tx.send(ScanEvent::Found(Category::OldAppLeftovers, Finding {
                    path: path.clone(), physical_size: bucket.physical, logical_size: bucket.logical,
                    last_modified: bucket.newest, owner_uid: bucket.uid, cloud_backed: false,
                    detail: bucket.detail.clone(),
                }));
            }
            Category::AppCache => {
                if bucket.physical < 5_000_000 { continue; }
                let _ = tx.send(ScanEvent::Found(Category::AppCache, Finding {
                    path: path.clone(), physical_size: bucket.physical, logical_size: bucket.logical,
                    last_modified: bucket.newest, owner_uid: bucket.uid, cloud_backed: false,
                    detail: String::new(),
                }));
            }
            Category::OldDownloads => {
                if bucket.physical < 500_000 { continue; }
                let _ = tx.send(ScanEvent::Found(Category::OldDownloads, Finding {
                    path: path.clone(), physical_size: bucket.physical, logical_size: bucket.logical,
                    last_modified: bucket.newest, owner_uid: bucket.uid, cloud_backed: false,
                    detail: bucket.detail.clone(),
                }));
            }
            Category::CloudSyncedLocal => {
                if bucket.physical < 1_000_000 { continue; }
                let _ = tx.send(ScanEvent::Found(Category::CloudSyncedLocal, Finding {
                    path: path.clone(), physical_size: bucket.physical, logical_size: bucket.logical,
                    last_modified: bucket.newest, owner_uid: bucket.uid, cloud_backed: true,
                    detail: format!("Synced to cloud — local copy uses {} on disk", ByteSize(bucket.physical)),
                }));
            }
            cat => {
                if bucket.physical < 1_000_000 { continue; }
                let _ = tx.send(ScanEvent::Found(cat.clone(), Finding {
                    path: path.clone(), physical_size: bucket.physical, logical_size: bucket.logical,
                    last_modified: bucket.newest, owner_uid: bucket.uid,
                    cloud_backed: bucket.cloud_backed, detail: bucket.detail.clone(),
                }));
            }
        }
    }

    // Phase 2: system dirs (small — sequential is fine)
    for sys_path in &["/private/tmp", "/private/var/log", "/Library/Caches", "/Library/Logs", "/cores"] {
        let p = Path::new(sys_path);
        if !p.exists() { continue; }
        let cat = if *sys_path == "/cores" { Category::CoreDumps }
            else if sys_path.contains("tmp") { Category::TmpFiles }
            else if sys_path.contains("log") || sys_path.contains("Logs") { Category::LogsAndDiagnostics }
            else { Category::AppCache };

        let (phys, logical, newest) = dir_physical_size(p);
        if phys > 1_000_000 {
            let _ = tx.send(ScanEvent::Found(cat, Finding {
                path: p.to_path_buf(), physical_size: phys, logical_size: logical,
                last_modified: newest, owner_uid: 0, cloud_backed: false, detail: String::new(),
            }));
        }
    }

    // Time Machine
    if let Ok(output) = std::process::Command::new("tmutil")
        .args(["listlocalsnapshots", "/"])
        .output()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let count = stdout.lines().filter(|l| l.contains("com.apple.TimeMachine")).count();
        if count > 0 {
            let _ = tx.send(ScanEvent::Found(Category::TimeMachineLocal, Finding {
                path: PathBuf::from(format!("{} local snapshots", count)),
                physical_size: 0, logical_size: 0, last_modified: None,
                owner_uid: 0, cloud_backed: false,
                detail: "Use 'sudo tmutil deletelocalsnapshots <date>' to remove".to_string(),
            }));
        }
    }

    let total_files = files_scanned.load(Ordering::Relaxed);
    let total_errors = perm_errors.load(Ordering::Relaxed);
    let total_dataless = dataless_skipped.load(Ordering::Relaxed);

    let _ = tx.send(ScanEvent::Complete(ScanResult {
        categories: Vec::new(),
        grand_total: 0,
        safe_total: 0,
        cloud_total: 0,
        files_scanned: total_files,
        perm_errors: total_errors,
        dataless_skipped: total_dataless,
        elapsed: start.elapsed(),
    }));
}

/// Single-pass scan: classify directories, then accumulate sizes from files
/// that jwalk visits *inside* those directories. No double-walk.
pub fn run_scan_path(root: &Path, tx: Sender<ScanEvent>) {
    use std::collections::HashMap;

    let start = Instant::now();
    let files_scanned = AtomicU64::new(0);
    let perm_errors = AtomicU64::new(0);
    let dataless_skipped = AtomicU64::new(0);

    let walker = WalkDir::new(root)
        .skip_hidden(false)
        .follow_links(false)
        .sort(false)
        .parallelism(jwalk::Parallelism::RayonNewPool(num_cpus::get().min(8)));

    // Buckets: classified dir path -> (category, physical_bytes, logical_bytes, newest_mtime, uid)
    struct Bucket {
        category: Category,
        physical: u64,
        logical: u64,
        newest: Option<SystemTime>,
        uid: u32,
        cloud_backed: bool,
    }
    let mut buckets: Vec<(PathBuf, Bucket)> = Vec::new();

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => { perm_errors.fetch_add(1, Ordering::Relaxed); continue; }
        };

        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type().is_dir();
        let depth = entry.depth();

        files_scanned.fetch_add(1, Ordering::Relaxed);

        // Check if this entry falls inside an existing bucket
        // (buckets are sorted by insertion order = discovery order during walk)
        let in_bucket = buckets.iter().position(|(prefix, _)| path.starts_with(prefix));

        if is_dir && depth > 0 && in_bucket.is_none() {
            // Try to classify this directory as a new bucket
            if let Some(cat) = classify_path(&path, &name, true, depth) {
                let uid = fs::symlink_metadata(&path).map(|m| m.uid()).unwrap_or(0);
                buckets.push((path.clone(), Bucket {
                    category: cat, physical: 0, logical: 0, newest: None, uid, cloud_backed: false,
                }));
                continue; // jwalk will descend into it, we'll accumulate from files
            }

            // Stale git projects
            if depth >= 1 && depth <= 5 && path.join(".git").is_dir() {
                let uid = fs::symlink_metadata(&path).map(|m| m.uid()).unwrap_or(0);
                buckets.push((path.clone(), Bucket {
                    category: Category::StaleProject, physical: 0, logical: 0,
                    newest: None, uid, cloud_backed: false,
                }));
                continue;
            }
        }

        // For files: accumulate into their bucket, or check for large files
        if entry.file_type().is_file() {
            if let Ok(meta) = path.symlink_metadata() {
                let flags = meta.st_flags() as u32;
                if flags & SF_DATALESS != 0 {
                    dataless_skipped.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                let phys = (meta.blocks() as u64) * 512;
                let logical = meta.len();
                let mtime = meta.modified().ok();

                if let Some(idx) = in_bucket {
                    // Accumulate into bucket
                    let bucket = &mut buckets[idx].1;
                    bucket.physical += phys;
                    bucket.logical += logical;
                    if let Some(mt) = mtime {
                        bucket.newest = Some(bucket.newest.map_or(mt, |n: SystemTime| n.max(mt)));
                    }
                } else if phys > 200_000_000 {
                    // Large file not in any bucket
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                    let cat = if is_vm_ext(&ext) { Category::VmImages }
                        else if is_media_ext(&ext) { Category::LargeMedia }
                        else { Category::LargeOther };
                    let _ = tx.send(ScanEvent::Found(cat, Finding {
                        path: path.clone(), physical_size: phys, logical_size: logical,
                        last_modified: mtime, owner_uid: meta.uid(),
                        cloud_backed: false, detail: ext,
                    }));
                }
            }
        }
    }

    // Emit findings from accumulated buckets
    let six_months_ago = SystemTime::now() - Duration::from_secs(180 * 24 * 3600);
    for (path, bucket) in &buckets {
        if bucket.category == Category::StaleProject {
            let is_stale = bucket.newest.map_or(true, |t| t < six_months_ago);
            if !is_stale || bucket.physical < 10_000_000 { continue; }
            let detail = check_git_dirty_fast(path);
            let _ = tx.send(ScanEvent::Found(Category::StaleProject, Finding {
                path: path.clone(), physical_size: bucket.physical, logical_size: bucket.logical,
                last_modified: bucket.newest, owner_uid: bucket.uid, cloud_backed: false, detail,
            }));
        } else {
            if bucket.physical < 1_000_000 { continue; }
            let _ = tx.send(ScanEvent::Found(bucket.category.clone(), Finding {
                path: path.clone(), physical_size: bucket.physical, logical_size: bucket.logical,
                last_modified: bucket.newest, owner_uid: bucket.uid,
                cloud_backed: bucket.cloud_backed, detail: String::new(),
            }));
        }
    }

    let _ = tx.send(ScanEvent::Complete(ScanResult {
        categories: Vec::new(), grand_total: 0, safe_total: 0, cloud_total: 0,
        files_scanned: files_scanned.load(Ordering::Relaxed),
        perm_errors: perm_errors.load(Ordering::Relaxed),
        dataless_skipped: dataless_skipped.load(Ordering::Relaxed),
        elapsed: start.elapsed(),
    }));
}

/// Single-pass scan using getattrlistbulk + rayon work-stealing at every level.
/// DashMap for lock-free bucket accumulation.
pub fn run_scan_bulk(root: &Path, tx: Sender<ScanEvent>) {
    use crate::scanner::bulkwalk;
    use dashmap::DashMap;

    let start = Instant::now();
    let files_scanned = AtomicU64::new(0);
    let dataless_skipped = AtomicU64::new(0);

    // DashMap keyed by bucket path — lock-free concurrent access
    let buckets: DashMap<PathBuf, BulkBucket> = DashMap::new();
    let tx_ref = &tx;

    bulkwalk::walk_bulk_parallel(root, |path, entry| {
        files_scanned.fetch_add(1, Ordering::Relaxed);
        let name = &entry.name;

        // Check bucket membership by walking ancestor paths (O(depth) not O(buckets))
        let parent_bucket = {
            let mut ancestor = path.parent();
            let mut found: Option<PathBuf> = None;
            while let Some(a) = ancestor {
                if a.as_os_str().len() < root.as_os_str().len() { break; }
                if buckets.contains_key(a) {
                    found = Some(a.to_path_buf());
                    break;
                }
                ancestor = a.parent();
            }
            found
        };

        if entry.is_dir {
            if parent_bucket.is_none() {
                let depth = path.strip_prefix(root).map(|p| p.components().count()).unwrap_or(0);

                if let Some(cat) = classify_path(path, name, true, depth) {
                    buckets.insert(path.to_path_buf(), BulkBucket {
                        category: cat, physical: 0, logical: 0,
                        newest: None, cloud_backed: false, detail: String::new(),
                    });
                    return true;
                }

                if depth >= 1 && depth <= 5 && path.join(".git").is_dir() {
                    buckets.insert(path.to_path_buf(), BulkBucket {
                        category: Category::StaleProject, physical: 0, logical: 0,
                        newest: None, cloud_backed: false, detail: String::new(),
                    });
                    return true;
                }
            }
            return true;
        }

        if entry.is_file {
            if entry.is_dataless {
                dataless_skipped.fetch_add(1, Ordering::Relaxed);
                return false;
            }

            let phys = entry.physical_size;
            let logical = entry.logical_size;

            if let Some(key) = parent_bucket {
                if let Some(mut bucket) = buckets.get_mut(&key) {
                    bucket.physical += phys;
                    bucket.logical += logical;
                    if let Some(mt) = entry.modified {
                        bucket.newest = Some(bucket.newest.map_or(mt, |n: SystemTime| n.max(mt)));
                    }
                }
            } else if phys > 200_000_000 {
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                let cat = if is_vm_ext(&ext) { Category::VmImages }
                    else if is_media_ext(&ext) { Category::LargeMedia }
                    else { Category::LargeOther };
                let cloud_backed = path.to_string_lossy().contains("CloudStorage")
                    || path.to_string_lossy().contains("Mobile Documents");
                let _ = tx_ref.send(ScanEvent::Found(cat, Finding {
                    path: path.to_path_buf(), physical_size: phys, logical_size: logical,
                    last_modified: entry.modified, owner_uid: 0,
                    cloud_backed, detail: ext,
                }));
            }
        }

        false
    });

    // Emit bucket findings
    let six_months_ago = SystemTime::now() - Duration::from_secs(180 * 24 * 3600);
    for entry in buckets.iter() {
        let (path, bucket) = (entry.key(), entry.value());
        if bucket.category == Category::StaleProject {
            let is_stale = bucket.newest.map_or(true, |t| t < six_months_ago);
            if !is_stale || bucket.physical < 10_000_000 { continue; }
            let detail = check_git_dirty_fast(path);
            let _ = tx.send(ScanEvent::Found(Category::StaleProject, Finding {
                path: path.clone(), physical_size: bucket.physical, logical_size: bucket.logical,
                last_modified: bucket.newest, owner_uid: 0, cloud_backed: false, detail,
            }));
        } else {
            if bucket.physical < 1_000_000 { continue; }
            let _ = tx.send(ScanEvent::Found(bucket.category.clone(), Finding {
                path: path.clone(), physical_size: bucket.physical, logical_size: bucket.logical,
                last_modified: bucket.newest, owner_uid: 0,
                cloud_backed: bucket.cloud_backed, detail: String::new(),
            }));
        }
    }

    let _ = tx.send(ScanEvent::Complete(ScanResult {
        categories: Vec::new(), grand_total: 0, safe_total: 0, cloud_total: 0,
        files_scanned: files_scanned.load(Ordering::Relaxed),
        perm_errors: 0,
        dataless_skipped: dataless_skipped.load(Ordering::Relaxed),
        elapsed: start.elapsed(),
    }));
}

struct BulkBucket {
    category: Category,
    physical: u64,
    logical: u64,
    newest: Option<SystemTime>,
    cloud_backed: bool,
    detail: String,
}

/// Fast full-disk scan using getattrlistbulk. Called by spawn_scan.
pub fn run_scan_fast(tx: Sender<ScanEvent>) {
    use crate::scanner::bulkwalk;

    let start = Instant::now();
    let files_scanned = AtomicU64::new(0);
    let dataless_skipped = AtomicU64::new(0);

    let _ = tx.send(ScanEvent::Progress(ScanProgress {
        phase: ScanPhase::DetectingApps,
        files_scanned: 0, perm_errors: 0, dataless_skipped: 0,
        elapsed: start.elapsed(),
    }));
    let installed_ids = get_installed_app_bundle_ids();

    let homes = get_user_homes();
    for home in &homes {
        let user = home.file_name().unwrap_or_default().to_string_lossy().to_string();
        let _ = tx.send(ScanEvent::Progress(ScanProgress {
            phase: ScanPhase::ScanningUser(user),
            files_scanned: files_scanned.load(Ordering::Relaxed),
            perm_errors: 0,
            dataless_skipped: dataless_skipped.load(Ordering::Relaxed),
            elapsed: start.elapsed(),
        }));

        let buckets: dashmap::DashMap<PathBuf, BulkBucket> = dashmap::DashMap::new();
        let cutoff_90d = SystemTime::now() - Duration::from_secs(90 * 24 * 3600);

        bulkwalk::walk_bulk_parallel(home, |path, entry| {
            files_scanned.fetch_add(1, Ordering::Relaxed);
            let name = &entry.name;

            let parent_bucket = {
                let mut ancestor = path.parent();
                let mut found: Option<PathBuf> = None;
                while let Some(a) = ancestor {
                    if a.as_os_str().len() < home.as_os_str().len() { break; }
                    if buckets.contains_key(a) {
                        found = Some(a.to_path_buf());
                        break;
                    }
                    ancestor = a.parent();
                }
                found
            };

            if entry.is_dir {
                if parent_bucket.is_none() {
                    let depth = path.strip_prefix(home).map(|p| p.components().count()).unwrap_or(0);

                    if let Some(cat) = classify_path(path, name, true, depth) {
                        if cat == Category::OldDownloads {
                            return true; // descend, children get bucketed by age
                        }
                        let cloud_backed = cat == Category::CloudSyncedLocal
                            && path.to_string_lossy().contains("Library/CloudStorage/");
                        buckets.insert(path.to_path_buf(), BulkBucket {
                            category: cat, physical: 0, logical: 0,
                            newest: None, cloud_backed, detail: String::new(),
                        });
                        return true;
                    }

                    if depth >= 2 && depth <= 5 && path.join(".git").is_dir() {
                        buckets.insert(path.to_path_buf(), BulkBucket {
                            category: Category::StaleProject, physical: 0, logical: 0,
                            newest: None, cloud_backed: false, detail: String::new(),
                        });
                        return true;
                    }

                    if path.to_string_lossy().contains("Library/Application Support/") && depth == 3 {
                        let bundle_like = name.replace(' ', ".").to_lowercase();
                        let is_installed = installed_ids.iter().any(|id| {
                            id.to_lowercase().contains(&bundle_like) || bundle_like.contains(&id.to_lowercase())
                        });
                        if !is_installed {
                            buckets.insert(path.to_path_buf(), BulkBucket {
                                category: Category::OldAppLeftovers, physical: 0, logical: 0,
                                newest: None, cloud_backed: false,
                                detail: "App may no longer be installed".to_string(),
                            });
                            return true;
                        }
                    }

                    if path.to_string_lossy().contains("Library/Caches/") && depth == 3 {
                        buckets.insert(path.to_path_buf(), BulkBucket {
                            category: Category::AppCache, physical: 0, logical: 0,
                            newest: None, cloud_backed: false, detail: String::new(),
                        });
                        return true;
                    }

                    // Old download child dirs
                    if depth == 2 && path.parent().map_or(false, |p| {
                        p.file_name().map_or(false, |n| n == "Downloads")
                    }) {
                        if let Some(mt) = entry.modified {
                            if mt < cutoff_90d {
                                buckets.insert(path.to_path_buf(), BulkBucket {
                                    category: Category::OldDownloads, physical: 0, logical: 0,
                                    newest: entry.modified, cloud_backed: false,
                                    detail: name.to_string(),
                                });
                                return true;
                            }
                        }
                    }
                }
                return true;
            }

            if entry.is_file {
                if entry.is_dataless {
                    dataless_skipped.fetch_add(1, Ordering::Relaxed);
                    return false;
                }
                let phys = entry.physical_size;
                let logical = entry.logical_size;

                if let Some(key) = parent_bucket {
                    if let Some(mut bucket) = buckets.get_mut(&key) {
                        bucket.physical += phys;
                        bucket.logical += logical;
                        if let Some(mt) = entry.modified {
                            bucket.newest = Some(bucket.newest.map_or(mt, |n: SystemTime| n.max(mt)));
                        }
                    }
                } else if phys > 200_000_000 {
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                    let cat = if is_vm_ext(&ext) { Category::VmImages }
                        else if is_media_ext(&ext) { Category::LargeMedia }
                        else { Category::LargeOther };
                    let cloud_backed = path.to_string_lossy().contains("CloudStorage")
                        || path.to_string_lossy().contains("Mobile Documents");
                    let _ = tx.send(ScanEvent::Found(cat, Finding {
                        path: path.to_path_buf(), physical_size: phys, logical_size: logical,
                        last_modified: entry.modified, owner_uid: 0,
                        cloud_backed, detail: ext,
                    }));
                } else if phys > 500_000 {
                    // Old download files
                    if path.parent().map_or(false, |p| p.file_name().map_or(false, |n| n == "Downloads")) {
                        if let Some(mt) = entry.modified {
                            if mt < cutoff_90d {
                                let _ = tx.send(ScanEvent::Found(Category::OldDownloads, Finding {
                                    path: path.to_path_buf(), physical_size: phys, logical_size: logical,
                                    last_modified: entry.modified, owner_uid: 0,
                                    cloud_backed: false, detail: name.to_string(),
                                }));
                            }
                        }
                    }
                }
            }
            false
        });

        // Emit bucket findings
        let six_months_ago = SystemTime::now() - Duration::from_secs(180 * 24 * 3600);
        for entry in buckets.iter() {
            let (bpath, bucket) = (entry.key(), entry.value());
            match &bucket.category {
                Category::StaleProject => {
                    let is_stale = bucket.newest.map_or(true, |t| t < six_months_ago);
                    if !is_stale || bucket.physical < 10_000_000 { continue; }
                    let detail = check_git_dirty_fast(bpath);
                    let _ = tx.send(ScanEvent::Found(Category::StaleProject, Finding {
                        path: bpath.clone(), physical_size: bucket.physical, logical_size: bucket.logical,
                        last_modified: bucket.newest, owner_uid: 0, cloud_backed: false, detail,
                    }));
                }
                Category::OldAppLeftovers => {
                    if bucket.physical < 10_000_000 { continue; }
                    let _ = tx.send(ScanEvent::Found(bucket.category.clone(), Finding {
                        path: bpath.clone(), physical_size: bucket.physical, logical_size: bucket.logical,
                        last_modified: bucket.newest, owner_uid: 0, cloud_backed: false,
                        detail: bucket.detail.clone(),
                    }));
                }
                Category::AppCache => {
                    if bucket.physical < 5_000_000 { continue; }
                    let _ = tx.send(ScanEvent::Found(bucket.category.clone(), Finding {
                        path: bpath.clone(), physical_size: bucket.physical, logical_size: bucket.logical,
                        last_modified: bucket.newest, owner_uid: 0, cloud_backed: false, detail: String::new(),
                    }));
                }
                Category::OldDownloads => {
                    if bucket.physical < 500_000 { continue; }
                    let _ = tx.send(ScanEvent::Found(bucket.category.clone(), Finding {
                        path: bpath.clone(), physical_size: bucket.physical, logical_size: bucket.logical,
                        last_modified: bucket.newest, owner_uid: 0, cloud_backed: false,
                        detail: bucket.detail.clone(),
                    }));
                }
                Category::CloudSyncedLocal => {
                    if bucket.physical < 1_000_000 { continue; }
                    let _ = tx.send(ScanEvent::Found(bucket.category.clone(), Finding {
                        path: bpath.clone(), physical_size: bucket.physical, logical_size: bucket.logical,
                        last_modified: bucket.newest, owner_uid: 0, cloud_backed: true,
                        detail: format!("Synced to cloud — {} on disk", ByteSize(bucket.physical)),
                    }));
                }
                cat => {
                    if bucket.physical < 1_000_000 { continue; }
                    let _ = tx.send(ScanEvent::Found(cat.clone(), Finding {
                        path: bpath.clone(), physical_size: bucket.physical, logical_size: bucket.logical,
                        last_modified: bucket.newest, owner_uid: 0,
                        cloud_backed: bucket.cloud_backed, detail: bucket.detail.clone(),
                    }));
                }
            }
        }
    }

    // System dirs
    let _ = tx.send(ScanEvent::Progress(ScanProgress {
        phase: ScanPhase::ScanningSystem,
        files_scanned: files_scanned.load(Ordering::Relaxed),
        perm_errors: 0,
        dataless_skipped: dataless_skipped.load(Ordering::Relaxed),
        elapsed: start.elapsed(),
    }));

    for sys_path in &["/private/tmp", "/private/var/log", "/Library/Caches", "/Library/Logs", "/cores"] {
        let p = Path::new(sys_path);
        if !p.exists() { continue; }
        let cat = if *sys_path == "/cores" { Category::CoreDumps }
            else if sys_path.contains("tmp") { Category::TmpFiles }
            else if sys_path.contains("log") || sys_path.contains("Logs") { Category::LogsAndDiagnostics }
            else { Category::AppCache };
        let (phys, logical, newest) = dir_physical_size(p);
        if phys > 1_000_000 {
            let _ = tx.send(ScanEvent::Found(cat, Finding {
                path: p.to_path_buf(), physical_size: phys, logical_size: logical,
                last_modified: newest, owner_uid: 0, cloud_backed: false, detail: String::new(),
            }));
        }
    }

    if let Ok(output) = std::process::Command::new("tmutil").args(["listlocalsnapshots", "/"]).output() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let count = stdout.lines().filter(|l| l.contains("com.apple.TimeMachine")).count();
        if count > 0 {
            let _ = tx.send(ScanEvent::Found(Category::TimeMachineLocal, Finding {
                path: PathBuf::from(format!("{} local snapshots", count)),
                physical_size: 0, logical_size: 0, last_modified: None,
                owner_uid: 0, cloud_backed: false,
                detail: "Use 'sudo tmutil deletelocalsnapshots <date>'".to_string(),
            }));
        }
    }

    let _ = tx.send(ScanEvent::Complete(ScanResult {
        categories: Vec::new(), grand_total: 0, safe_total: 0, cloud_total: 0,
        files_scanned: files_scanned.load(Ordering::Relaxed),
        perm_errors: 0,
        dataless_skipped: dataless_skipped.load(Ordering::Relaxed),
        elapsed: start.elapsed(),
    }));
}
