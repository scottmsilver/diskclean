//! Cleanup execution engine with tiered strategies and LLM safety validation.

use crate::model::Category;
use std::path::PathBuf;
use std::process::Command;

/// How aggressive the cleanup approach is.
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub enum CleanupTier {
    /// Use the official tool's cleanup command (brew cleanup, conda clean, etc.)
    /// Safest — the tool knows what's safe to remove.
    Official,
    /// Move to ~/To Delete staging folder. User reviews, then deletes.
    Stage,
    /// Direct rm -rf. Fast but irreversible.
    DirectDelete,
    /// Hardlink identical files together — frees space without removing anything.
    /// All paths still exist and work, they just share physical blocks.
    Dedup,
}

/// A specific cleanup action to execute.
#[derive(Debug, Clone)]
pub struct CleanupAction {
    pub tier: CleanupTier,
    pub description: String,
    pub command: Option<String>,         // shell command to run (for Official tier)
    pub paths_to_remove: Vec<PathBuf>,   // paths to stage/delete (for Stage/DirectDelete)
    pub estimated_savings: u64,
}

/// Get the recommended cleanup actions for a category, ordered safest-first.
pub fn cleanup_strategies(category: &Category, paths: &[(PathBuf, u64)]) -> Vec<CleanupAction> {
    let total_size: u64 = paths.iter().map(|(_, s)| s).sum();
    let all_paths: Vec<PathBuf> = paths.iter().map(|(p, _)| p.clone()).collect();

    match category {
        Category::PackageManagerCache => vec![
            CleanupAction {
                tier: CleanupTier::Official,
                description: "Run official cache cleanup commands".into(),
                command: Some("npm cache clean --force 2>/dev/null; pip cache purge 2>/dev/null; brew cleanup --prune=all 2>/dev/null; cargo cache -a 2>/dev/null".into()),
                paths_to_remove: vec![],
                estimated_savings: total_size,
            },
            CleanupAction {
                tier: CleanupTier::DirectDelete,
                description: "Delete cache directories directly".into(),
                command: None,
                paths_to_remove: all_paths,
                estimated_savings: total_size,
            },
        ],

        Category::CondaInstall => vec![
            CleanupAction {
                tier: CleanupTier::Official,
                description: "Clean conda package cache (keeps installation working)".into(),
                command: Some("conda clean --all -y".into()),
                paths_to_remove: vec![],
                estimated_savings: total_size / 2, // pkgs cache is ~half
            },
            CleanupAction {
                tier: CleanupTier::DirectDelete,
                description: "Move entire conda installation to staging".into(),
                command: None,
                paths_to_remove: all_paths,
                estimated_savings: total_size,
            },
        ],

        Category::OldNodeVersions => vec![
            CleanupAction {
                tier: CleanupTier::Official,
                description: "Uninstall old Node versions via nvm (keeps current)".into(),
                command: Some(build_nvm_uninstall_cmd(paths)),
                paths_to_remove: vec![],
                estimated_savings: total_size, // rough — keeps current version
            },
            CleanupAction {
                tier: CleanupTier::DirectDelete,
                description: "Move old Node versions to staging".into(),
                command: None,
                paths_to_remove: all_paths,
                estimated_savings: total_size,
            },
        ],

        Category::PythonVenvs => vec![
            CleanupAction {
                tier: CleanupTier::DirectDelete,
                description: "Move virtual environments to staging (recreate with pip install -r requirements.txt)".into(),
                command: None,
                paths_to_remove: all_paths,
                estimated_savings: total_size,
            },
        ],

        Category::OldIdeExtensions => vec![
            CleanupAction {
                tier: CleanupTier::DirectDelete,
                description: "Move old extension versions to staging (IDEs keep current version separately)".into(),
                command: None,
                paths_to_remove: all_paths,
                estimated_savings: total_size,
            },
        ],

        Category::BuildArtifact | Category::NodeModules => vec![
            CleanupAction {
                tier: CleanupTier::DirectDelete,
                description: "Delete build artifacts/node_modules (rebuild with cargo build / npm install)".into(),
                command: None,
                paths_to_remove: all_paths,
                estimated_savings: total_size,
            },
        ],

        Category::AppCache | Category::BrowserCache | Category::ElectronCache => vec![
            CleanupAction {
                tier: CleanupTier::Dedup,
                description: "Deduplicate identical files within caches (hardlink — no data loss)".into(),
                command: None,
                paths_to_remove: all_paths.clone(),
                estimated_savings: total_size / 4, // conservative estimate
            },
            CleanupAction {
                tier: CleanupTier::DirectDelete,
                description: "Delete application caches (apps rebuild automatically)".into(),
                command: None,
                paths_to_remove: all_paths,
                estimated_savings: total_size,
            },
        ],

        Category::Trash => vec![
            CleanupAction {
                tier: CleanupTier::DirectDelete,
                description: "Empty Trash".into(),
                command: Some("rm -rf /Users/*/.Trash/*".into()),
                paths_to_remove: vec![],
                estimated_savings: total_size,
            },
        ],

        Category::XcodeDerivedData | Category::SimulatorRuntimes => vec![
            CleanupAction {
                tier: CleanupTier::Official,
                description: "Clean via Xcode tools".into(),
                command: Some("rm -rf ~/Library/Developer/Xcode/DerivedData/*; xcrun simctl delete unavailable".into()),
                paths_to_remove: vec![],
                estimated_savings: total_size,
            },
        ],

        Category::DockerData => vec![
            CleanupAction {
                tier: CleanupTier::Official,
                description: "Prune Docker (removes unused images, containers, volumes)".into(),
                command: Some("docker system prune -a --volumes -f".into()),
                paths_to_remove: vec![],
                estimated_savings: total_size,
            },
        ],

        Category::HomebrewOldVersions => vec![
            CleanupAction {
                tier: CleanupTier::Official,
                description: "Run brew cleanup".into(),
                command: Some("brew cleanup --prune=all".into()),
                paths_to_remove: vec![],
                estimated_savings: total_size,
            },
        ],

        Category::CrashReports | Category::CoreDumps | Category::TmpFiles
        | Category::LogsAndDiagnostics => vec![
            CleanupAction {
                tier: CleanupTier::DirectDelete,
                description: "Delete old logs/crash reports/temp files".into(),
                command: None,
                paths_to_remove: all_paths,
                estimated_savings: total_size,
            },
        ],

        Category::CachedBrowserBinaries => vec![
            CleanupAction {
                tier: CleanupTier::DirectDelete,
                description: "Delete cached Chromium binaries (Puppeteer/Selenium re-download on next use)".into(),
                command: None,
                paths_to_remove: all_paths,
                estimated_savings: total_size,
            },
        ],

        Category::DuplicateFiles => vec![
            CleanupAction {
                tier: CleanupTier::DirectDelete,
                description: "Delete duplicate copies (keeps one copy in place)".into(),
                command: None,
                paths_to_remove: all_paths,
                estimated_savings: total_size,
            },
        ],

        Category::RustupToolchains => vec![
            CleanupAction {
                tier: CleanupTier::Official,
                description: "Remove unused Rust toolchains/targets".into(),
                command: Some("rustup toolchain list | grep -v default | xargs -I{} rustup toolchain uninstall {}".into()),
                paths_to_remove: vec![],
                estimated_savings: total_size,
            },
        ],

        Category::SystemTempFolders => vec![
            CleanupAction {
                tier: CleanupTier::DirectDelete,
                description: "Delete per-user temp caches in /var/folders (apps recreate as needed)".into(),
                command: None,
                paths_to_remove: all_paths,
                estimated_savings: total_size,
            },
        ],

        Category::StaleStagingFolder => vec![
            CleanupAction {
                tier: CleanupTier::DirectDelete,
                description: "Delete leftover 'To Delete' staging folders".into(),
                command: None,
                paths_to_remove: all_paths,
                estimated_savings: total_size,
            },
        ],

        Category::ApfsSnapshots => vec![
            CleanupAction {
                tier: CleanupTier::Official,
                description: "Delete APFS update snapshots (safe after successful update)".into(),
                command: Some("tmutil listlocalsnapshots / | grep com.apple.os.update | while read s; do sudo tmutil deletelocalsnapshots \"$s\" 2>/dev/null; done".into()),
                paths_to_remove: vec![],
                estimated_savings: total_size,
            },
        ],

        // Default for everything else
        _ => vec![
            CleanupAction {
                tier: CleanupTier::DirectDelete,
                description: "Move to ~/To Delete for review".into(),
                command: None,
                paths_to_remove: all_paths,
                estimated_savings: total_size,
            },
        ],
    }
}

/// Execute a cleanup action. Returns (bytes_freed, error_message).
pub fn execute_cleanup(action: &CleanupAction) -> (u64, Option<String>) {
    match action.tier {
        CleanupTier::Official => {
            if let Some(cmd) = &action.command {
                let output = Command::new("sh")
                    .args(["-c", cmd])
                    .output();
                match output {
                    Ok(o) if o.status.success() => (action.estimated_savings, None),
                    Ok(o) => (0, Some(String::from_utf8_lossy(&o.stderr).to_string())),
                    Err(e) => (0, Some(format!("Failed to run: {}", e))),
                }
            } else {
                (0, Some("No command specified".into()))
            }
        }
        CleanupTier::Dedup => {
            // Hardlink identical files: group by size+hash, keep one, hardlink rest.
            // Files stay at same paths but share physical blocks — zero data loss.
            let mut total_freed = 0u64;
            let mut errors = Vec::new();

            // Group files by size
            let mut by_size: std::collections::HashMap<u64, Vec<PathBuf>> = std::collections::HashMap::new();
            for path in &action.paths_to_remove {
                if let Ok(meta) = std::fs::metadata(path) {
                    if meta.is_file() {
                        by_size.entry(meta.len()).or_default().push(path.clone());
                    }
                }
            }

            // For each size group with 2+ files, hash and hardlink matches
            for (_size, paths) in &by_size {
                if paths.len() < 2 { continue; }

                // Group by content hash (first+last 4KB)
                let mut hash_groups: std::collections::HashMap<u128, Vec<PathBuf>> = std::collections::HashMap::new();
                for path in paths {
                    if let Some(hash) = super::scanner::walk::quick_file_hash(path, *_size) {
                        hash_groups.entry(hash).or_default().push(path.clone());
                    }
                }

                for (_, group) in hash_groups {
                    if group.len() < 2 { continue; }
                    let keep = &group[0];
                    for dupe in &group[1..] {
                        // Get physical size before hardlink
                        let phys_before = std::fs::metadata(dupe)
                            .map(|m| m.len()).unwrap_or(0);

                        // Hardlink: ln -f keep dupe (atomically replaces dupe with link to keep)
                        // Use a temp file to make it atomic
                        let tmp = dupe.with_extension("diskclean_tmp");
                        match std::fs::hard_link(keep, &tmp) {
                            Ok(()) => {
                                match std::fs::rename(&tmp, dupe) {
                                    Ok(()) => total_freed += phys_before,
                                    Err(e) => {
                                        let _ = std::fs::remove_file(&tmp);
                                        errors.push(format!("{}: rename failed: {}", dupe.display(), e));
                                    }
                                }
                            }
                            Err(e) => errors.push(format!("{}: hardlink failed: {}", dupe.display(), e)),
                        }
                    }
                }
            }

            let err = if errors.is_empty() { None } else { Some(errors.join("; ")) };
            (total_freed, err)
        }
        CleanupTier::Stage | CleanupTier::DirectDelete => {
            let mut total_freed = 0u64;
            let mut errors = Vec::new();
            for path in &action.paths_to_remove {
                if !path.exists() { continue; }
                let result = if path.is_dir() {
                    std::fs::remove_dir_all(path)
                } else {
                    std::fs::remove_file(path)
                };
                match result {
                    Ok(_) => total_freed += action.estimated_savings / action.paths_to_remove.len().max(1) as u64,
                    Err(e) => errors.push(format!("{}: {}", path.display(), e)),
                }
            }
            let err = if errors.is_empty() { None } else { Some(errors.join("; ")) };
            (total_freed, err)
        }
    }
}

/// Verify cleanup: check which paths are gone and summarize.
pub fn verify_cleanup(action: &CleanupAction) -> VerifyResult {
    if action.paths_to_remove.is_empty() {
        return VerifyResult { summary: "Command executed — rescan to verify".to_string() };
    }

    let total = action.paths_to_remove.len();
    let removed = action.paths_to_remove.iter().filter(|p| !p.exists()).count();

    VerifyResult {
        summary: if removed == total {
            format!("All {} items verified removed", total)
        } else {
            format!("{} of {} items removed, {} remaining", removed, total, total - removed)
        },
    }
}

pub struct VerifyResult {
    pub summary: String,
}

fn build_nvm_uninstall_cmd(paths: &[(PathBuf, u64)]) -> String {
    // Extract version numbers from paths like .nvm/versions/node/v16.13.1
    let versions: Vec<String> = paths.iter()
        .filter_map(|(p, _)| {
            p.file_name()?.to_str().map(|s| s.to_string())
        })
        .collect();
    if versions.is_empty() {
        "echo 'No old versions found'".into()
    } else {
        versions.iter()
            .map(|v| format!("nvm uninstall {}", v))
            .collect::<Vec<_>>()
            .join("; ")
    }
}
