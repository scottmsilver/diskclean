#![allow(dead_code)]
use super::category::Category;
use super::finding::Finding;
use std::time::Duration;

#[derive(Debug, Clone)]
pub enum ScanPhase {
    DetectingApps,
    ScanningUser(String),
    ScanningSystem,
    Complete,
}

#[derive(Debug, Clone)]
pub struct ScanProgress {
    pub phase: ScanPhase,
    pub files_scanned: u64,
    pub perm_errors: u64,
    pub dataless_skipped: u64,
    pub elapsed: Duration,
}

pub struct CategoryResult {
    pub category: Category,
    pub total_size: u64,
    pub findings: Vec<Finding>,
}

pub struct ScanResult {
    pub categories: Vec<CategoryResult>,
    pub grand_total: u64,
    pub safe_total: u64,
    pub cloud_total: u64,
    pub files_scanned: u64,
    pub perm_errors: u64,
    pub dataless_skipped: u64,
    pub elapsed: Duration,
}

pub enum ScanEvent {
    Progress(ScanProgress),
    Found(Category, Finding),
    Complete(ScanResult),
}
