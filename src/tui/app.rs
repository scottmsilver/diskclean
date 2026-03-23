use crate::cleanup::{self, CleanupAction};
use crate::cleanup_queue::CleanupQueue;
use crate::model::*;
use crate::safety_oracle;
use crate::staging::StagingDir;
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

pub enum Screen {
    Scanning,
    Results,
}

#[derive(Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Dialog {
    None,
    ConfirmStage,                      // legacy: "Move N items (X) to ~/To Delete?"
    StageResult(String),               // legacy: Success/error message
    CleanupPicker,                     // show cleanup strategies for selected category
    LlmAssessing,                      // "Asking Gemini..."
    LlmResult(LlmAssessmentResult),   // show LLM verdict
    CleanupConfirm(usize),            // confirm executing strategy index
    CleanupRunning,                    // "Cleaning up..."
    CleanupDone(CleanupResult),       // show what was cleaned + verification
}

#[derive(Clone, PartialEq, Eq)]
pub struct LlmAssessmentResult {
    pub safe: bool,
    pub confidence: String,
    pub reasoning: String,
    pub warnings: Vec<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct CleanupResult {
    pub strategy: String,
    pub bytes_freed: u64,
    pub error: Option<String>,
    pub verification: String,
}

pub struct CategoryRow {
    pub category: Category,
    pub total_size: u64,
    pub findings: Vec<Finding>,
}

pub struct App {
    pub screen: Screen,

    // Scanning phase
    pub progress: ScanProgress,
    pub live_categories: BTreeMap<Category, Vec<Finding>>,
    pub spinner_tick: usize,

    // Results phase
    pub categories: Vec<CategoryRow>,
    pub selected: usize,
    pub expanded: HashSet<usize>,
    pub grand_total: u64,
    pub safe_total: u64,
    pub cloud_total: u64,
    pub scan_stats: Option<ScanResult>,

    // Staging (delete)
    pub marked: HashSet<(usize, Option<usize>)>, // (cat_idx, finding_idx)
    pub marked_size: u64,
    pub dialog: Dialog,
    pub staging: StagingDir,
    pub staged_count: usize,  // items moved so far this session
    pub staged_size: u64,

    // Cleanup engine
    pub cleanup_strategies: Vec<CleanupAction>,  // strategies for current selection
    pub cleanup_selected_strategy: usize,        // which strategy is highlighted
    pub cleanup_queue: CleanupQueue,             // async job queue
    pub show_jobs: bool,                         // toggle jobs panel

    pub should_quit: bool,
    pub show_help: bool,
}

impl App {
    pub fn new() -> Self {
        Self {
            screen: Screen::Scanning,
            progress: ScanProgress {
                phase: ScanPhase::DetectingApps,
                files_scanned: 0,
                perm_errors: 0,
                dataless_skipped: 0,
                elapsed: Duration::ZERO,
            },
            live_categories: BTreeMap::new(),
            spinner_tick: 0,
            categories: Vec::new(),
            selected: 0,
            expanded: HashSet::new(),
            grand_total: 0,
            safe_total: 0,
            cloud_total: 0,
            scan_stats: None,
            marked: HashSet::new(),
            marked_size: 0,
            dialog: Dialog::None,
            staging: StagingDir::new(),
            staged_count: 0,
            staged_size: 0,
            cleanup_strategies: Vec::new(),
            cleanup_selected_strategy: 0,
            cleanup_queue: CleanupQueue::new(),
            show_jobs: false,
            should_quit: false,
            show_help: false,
        }
    }

    pub fn tick(&mut self) {
        self.spinner_tick = self.spinner_tick.wrapping_add(1);
    }

    pub fn on_found(&mut self, cat: Category, finding: Finding) {
        self.live_categories.entry(cat).or_default().push(finding);
    }

    pub fn on_progress(&mut self, progress: ScanProgress) {
        self.progress = progress;
    }

    pub fn on_complete(&mut self, result: ScanResult) {
        let mut cats: Vec<CategoryRow> = Vec::new();

        for (cat, mut findings) in std::mem::take(&mut self.live_categories) {
            let total_size: u64 = findings.iter().map(|f| f.physical_size).sum();
            findings.sort_by(|a, b| b.physical_size.cmp(&a.physical_size));
            cats.push(CategoryRow { category: cat, total_size, findings });
        }

        cats.sort_by(|a, b| b.total_size.cmp(&a.total_size));

        self.grand_total = cats.iter().map(|c| c.total_size).sum();
        self.safe_total = cats.iter()
            .filter(|c| c.category.risk_level() == RiskLevel::Safe)
            .map(|c| c.total_size).sum();
        self.cloud_total = cats.iter()
            .filter(|c| c.category == Category::CloudSyncedLocal)
            .map(|c| c.total_size).sum();

        self.categories = cats;
        self.scan_stats = Some(result);
        self.screen = Screen::Results;
        self.selected = 0;
    }

    pub fn visible_row_count(&self) -> usize {
        let mut count = 0;
        for (i, cat) in self.categories.iter().enumerate() {
            count += 1;
            if self.expanded.contains(&i) {
                count += cat.findings.len().min(20);
            }
        }
        count
    }

    pub fn selection_to_indices(&self) -> Option<(usize, Option<usize>)> {
        let mut row = 0;
        for (i, cat) in self.categories.iter().enumerate() {
            if row == self.selected { return Some((i, None)); }
            row += 1;
            if self.expanded.contains(&i) {
                let n = cat.findings.len().min(20);
                for fi in 0..n {
                    if row == self.selected { return Some((i, Some(fi))); }
                    row += 1;
                }
            }
        }
        None
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 { self.selected -= 1; }
    }

    pub fn move_down(&mut self) {
        let max = self.visible_row_count();
        if max > 0 && self.selected < max - 1 { self.selected += 1; }
    }

    pub fn toggle_expand(&mut self) {
        if let Some((ci, None)) = self.selection_to_indices() {
            if self.expanded.contains(&ci) {
                self.expanded.remove(&ci);
            } else {
                self.expanded.insert(ci);
            }
        }
    }

    pub fn home(&mut self) { self.selected = 0; }

    pub fn end(&mut self) {
        let max = self.visible_row_count();
        if max > 0 { self.selected = max - 1; }
    }

    pub fn selected_category(&self) -> Option<&CategoryRow> {
        self.selection_to_indices().map(|(ci, _)| &self.categories[ci])
    }

    pub fn selected_finding(&self) -> Option<(&CategoryRow, &Finding)> {
        match self.selection_to_indices() {
            Some((ci, Some(fi))) => Some((&self.categories[ci], &self.categories[ci].findings[fi])),
            _ => None,
        }
    }

    // ── Mark / unmark for deletion ──────────────────────────────────────

    pub fn toggle_mark(&mut self) {
        let Some(indices) = self.selection_to_indices() else { return };

        match indices {
            (ci, None) => {
                // Toggle all findings in this category
                let n = self.categories[ci].findings.len().min(20);
                let all_marked = (0..n).all(|fi| self.marked.contains(&(ci, Some(fi))));
                for fi in 0..n {
                    let key = (ci, Some(fi));
                    if all_marked {
                        if self.marked.remove(&key) {
                            self.marked_size -= self.categories[ci].findings[fi].physical_size;
                        }
                    } else if self.marked.insert(key) {
                        self.marked_size += self.categories[ci].findings[fi].physical_size;
                    }
                }
            }
            (ci, Some(fi)) => {
                let key = (ci, Some(fi));
                if self.marked.contains(&key) {
                    self.marked.remove(&key);
                    self.marked_size -= self.categories[ci].findings[fi].physical_size;
                } else {
                    self.marked.insert(key);
                    self.marked_size += self.categories[ci].findings[fi].physical_size;
                }
            }
        }
    }

    pub fn is_marked(&self, ci: usize, fi: Option<usize>) -> bool {
        match fi {
            Some(fi) => self.marked.contains(&(ci, Some(fi))),
            None => {
                // Category is "marked" if all its visible findings are marked
                let n = self.categories[ci].findings.len().min(20);
                n > 0 && (0..n).all(|f| self.marked.contains(&(ci, Some(f))))
            }
        }
    }

    pub fn request_stage(&mut self) {
        if self.marked.is_empty() { return; }
        self.dialog = Dialog::ConfirmStage;
    }

    /// Execute the staging: move all marked items to ~/To Delete/
    pub fn execute_stage(&mut self) {
        let mut moved = 0usize;
        let mut moved_size = 0u64;
        let mut errors: Vec<String> = Vec::new();

        // Collect paths to move (we need to collect first since we'll mutate)
        let mut to_move: Vec<(usize, usize, PathBuf, u64)> = Vec::new();
        for &(ci, fi_opt) in &self.marked {
            if let Some(fi) = fi_opt {
                if ci < self.categories.len() && fi < self.categories[ci].findings.len() {
                    let f = &self.categories[ci].findings[fi];
                    to_move.push((ci, fi, f.path.clone(), f.physical_size));
                }
            }
        }

        // Sort by (ci, fi) descending so removals don't shift indices
        to_move.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));

        for (_ci, _fi, path, size) in &to_move {
            match self.staging.stage(&path) {
                Ok(_dest) => {
                    moved += 1;
                    moved_size += size;
                }
                Err(e) => {
                    errors.push(format!("{}: {}", path.display(), e));
                }
            }
        }

        // Remove moved items from findings (reverse order to keep indices valid)
        let mut removal_indices: Vec<(usize, usize)> = to_move.iter()
            .filter(|(_, _, path, _)| {
                // Only remove if the source is actually gone
                !path.exists()
            })
            .map(|(ci, fi, _, _)| (*ci, *fi))
            .collect();
        removal_indices.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));

        for (ci, fi) in removal_indices {
            if ci < self.categories.len() && fi < self.categories[ci].findings.len() {
                let removed = self.categories[ci].findings.remove(fi);
                self.categories[ci].total_size -= removed.physical_size;
            }
        }

        // Recalculate totals
        self.grand_total = self.categories.iter().map(|c| c.total_size).sum();
        self.safe_total = self.categories.iter()
            .filter(|c| c.category.risk_level() == RiskLevel::Safe)
            .map(|c| c.total_size).sum();

        // Clear marks
        self.marked.clear();
        self.marked_size = 0;
        self.staged_count += moved;
        self.staged_size += moved_size;

        // Show result
        let msg = if errors.is_empty() {
            format!("Moved {} items ({}) to {}", moved, bytesize::ByteSize(moved_size), self.staging.path.display())
        } else {
            format!("Moved {} items, {} errors. First error: {}", moved, errors.len(), errors[0])
        };
        self.dialog = Dialog::StageResult(msg);
    }

    // ── Cleanup workflow ────────────────────────────────────────────

    /// Open cleanup strategy picker for the selected category.
    pub fn open_cleanup_picker(&mut self) {
        let Some((ci, _fi)) = self.selection_to_indices() else { return };
        let cat_row = &self.categories[ci];

        let paths: Vec<(std::path::PathBuf, u64)> = cat_row.findings.iter()
            .map(|f| (f.path.clone(), f.physical_size))
            .collect();

        self.cleanup_strategies = cleanup::cleanup_strategies(&cat_row.category, &paths);
        self.cleanup_selected_strategy = 0;
        self.dialog = Dialog::CleanupPicker;
    }

    /// Add selected strategy to the job queue (doesn't execute yet).
    pub fn queue_cleanup(&mut self) {
        let idx = self.cleanup_selected_strategy;
        if idx >= self.cleanup_strategies.len() { return; }

        let strategy = self.cleanup_strategies[idx].clone();
        let cat_name = self.selection_to_indices()
            .map(|(ci, _)| self.categories[ci].category.label().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        self.cleanup_queue.enqueue(&cat_name, strategy);
        self.show_jobs = true;
        self.dialog = Dialog::None;
    }

    /// Move strategy selection up/down in the picker.
    pub fn cleanup_picker_up(&mut self) {
        if self.cleanup_selected_strategy > 0 {
            self.cleanup_selected_strategy -= 1;
        }
    }

    pub fn cleanup_picker_down(&mut self) {
        if self.cleanup_selected_strategy + 1 < self.cleanup_strategies.len() {
            self.cleanup_selected_strategy += 1;
        }
    }

    /// Ask LLM to assess the selected strategy (if API key available).
    pub fn assess_with_llm(&mut self) {
        let Some((ci, _)) = self.selection_to_indices() else { return };
        let cat_row = &self.categories[ci];
        let strategy = &self.cleanup_strategies[self.cleanup_selected_strategy];

        let path_str = if strategy.paths_to_remove.is_empty() {
            strategy.command.as_deref().unwrap_or("(command)").to_string()
        } else {
            strategy.paths_to_remove.first()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        };

        self.dialog = Dialog::LlmAssessing;

        // Try calling the oracle
        match safety_oracle::assess_safety(
            cat_row.category.label(),
            &path_str,
            strategy.estimated_savings,
            &strategy.description,
            cat_row.category.advice(),
        ) {
            Ok(assessment) => {
                self.dialog = Dialog::LlmResult(LlmAssessmentResult {
                    safe: assessment.safe,
                    confidence: format!("{:.0}%", assessment.confidence * 100.0),
                    reasoning: assessment.reasoning,
                    warnings: assessment.warnings,
                });
            }
            Err(e) => {
                self.dialog = Dialog::LlmResult(LlmAssessmentResult {
                    safe: false,
                    confidence: "N/A".into(),
                    reasoning: format!("LLM unavailable: {}", e),
                    warnings: vec!["Set GEMINI_API_KEY to enable AI safety checks".into()],
                });
            }
        }
    }

    /// Confirm and execute the selected cleanup strategy.
    pub fn confirm_cleanup(&mut self) {
        self.dialog = Dialog::CleanupConfirm(self.cleanup_selected_strategy);
    }

    /// Enqueue the cleanup job (runs in background, non-blocking).
    pub fn execute_cleanup(&mut self) {
        let idx = match &self.dialog {
            Dialog::CleanupConfirm(i) => *i,
            _ => return,
        };

        if idx >= self.cleanup_strategies.len() { return; }

        let strategy = self.cleanup_strategies[idx].clone();
        let cat_name = self.selection_to_indices()
            .map(|(ci, _)| self.categories[ci].category.label().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        // Enqueue — runs async in background thread
        let _job_id = self.cleanup_queue.enqueue(&cat_name, strategy);

        // Show jobs panel and dismiss dialog
        self.show_jobs = true;
        self.dialog = Dialog::None;
    }

    /// Refresh category sizes based on what jobs have cleaned.
    /// Called periodically from the TUI tick.
    pub fn refresh_after_cleanup(&mut self) {
        // Only refresh if there are completed jobs
        let freed = self.cleanup_queue.total_freed();
        if freed == 0 { return; }

        // Re-check which findings still exist
        for cat in &mut self.categories {
            cat.findings.retain(|f| f.path.exists());
            cat.total_size = cat.findings.iter().map(|f| f.physical_size).sum();
        }
        self.categories.retain(|c| c.total_size > 0 || c.category == Category::TimeMachineLocal);

        self.grand_total = self.categories.iter().map(|c| c.total_size).sum();
        self.safe_total = self.categories.iter()
            .filter(|c| c.category.risk_level() == RiskLevel::Safe)
            .map(|c| c.total_size).sum();
    }
}
