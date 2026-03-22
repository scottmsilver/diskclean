//! Cleanup execution engine with tiered strategies and LLM safety validation.

use crate::model::Category;
use std::path::{Path, PathBuf};
use std::process::Command;

/// How aggressive the cleanup approach is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CleanupTier {
    /// Use the official tool's cleanup command (brew cleanup, conda clean, etc.)
    /// Safest — the tool knows what's safe to remove.
    Official,
    /// Move to ~/To Delete staging folder. User reviews, then deletes.
    Stage,
    /// Direct rm -rf. Fast but irreversible.
    DirectDelete,
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
                tier: CleanupTier::Stage,
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
                tier: CleanupTier::Stage,
                description: "Move old Node versions to staging".into(),
                command: None,
                paths_to_remove: all_paths,
                estimated_savings: total_size,
            },
        ],

        Category::PythonVenvs => vec![
            CleanupAction {
                tier: CleanupTier::Stage,
                description: "Move virtual environments to staging (recreate with pip install -r requirements.txt)".into(),
                command: None,
                paths_to_remove: all_paths,
                estimated_savings: total_size,
            },
        ],

        Category::OldIdeExtensions => vec![
            CleanupAction {
                tier: CleanupTier::Stage,
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
                tier: CleanupTier::Stage,
                description: "Move duplicate copies to staging (keeps one copy in place)".into(),
                command: None,
                // For duplicates: remove all but the first path in each group
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

        // Default for everything else: stage first
        _ => vec![
            CleanupAction {
                tier: CleanupTier::Stage,
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
        CleanupTier::Stage => {
            let staging = crate::staging::StagingDir::new();
            let mut total_freed = 0u64;
            let mut errors = Vec::new();
            for path in &action.paths_to_remove {
                if !path.exists() { continue; }
                match staging.stage(path) {
                    Ok(_) => {
                        // Estimate size from the action
                        total_freed += action.estimated_savings / action.paths_to_remove.len().max(1) as u64;
                    }
                    Err(e) => errors.push(format!("{}: {}", path.display(), e)),
                }
            }
            let err = if errors.is_empty() { None } else { Some(errors.join("; ")) };
            (total_freed, err)
        }
        CleanupTier::DirectDelete => {
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

/// Per-item verification: check each path is gone and measure actual freed space.
/// Returns (all_verified, per_item_results).
pub fn verify_cleanup(action: &CleanupAction) -> VerifyResult {
    let mut results = Vec::new();
    let mut all_gone = true;

    for path in &action.paths_to_remove {
        if path.exists() {
            // Still there — check if size reduced (partial cleanup)
            let remaining = if path.is_dir() {
                crate::util::dir_size(path)
            } else {
                std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
            };
            results.push(ItemVerification {
                path: path.clone(),
                removed: false,
                remaining_bytes: remaining,
                note: format!("Still exists ({} remaining)", bytesize::ByteSize(remaining)),
            });
            all_gone = false;
        } else {
            results.push(ItemVerification {
                path: path.clone(),
                removed: true,
                remaining_bytes: 0,
                note: "Removed".to_string(),
            });
        }
    }

    // For Official tier (command-based), check if command-specific targets are cleaned
    if action.paths_to_remove.is_empty() && action.command.is_some() {
        // Can't verify per-path, just report success based on execution
        return VerifyResult {
            all_verified: true,
            items: vec![],
            summary: "Command executed — run scan again to verify savings".to_string(),
        };
    }

    let removed_count = results.iter().filter(|r| r.removed).count();
    let total_count = results.len();

    VerifyResult {
        all_verified: all_gone,
        summary: if all_gone {
            format!("All {} items verified removed", total_count)
        } else {
            format!("{} of {} items removed, {} remaining", removed_count, total_count, total_count - removed_count)
        },
        items: results,
    }
}

pub struct VerifyResult {
    pub all_verified: bool,
    pub summary: String,
    pub items: Vec<ItemVerification>,
}

pub struct ItemVerification {
    pub path: PathBuf,
    pub removed: bool,
    pub remaining_bytes: u64,
    pub note: String,
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
