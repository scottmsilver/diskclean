//! Async cleanup job queue. Jobs execute in background, TUI polls status.

use crate::cleanup::{self, CleanupAction, CleanupTier};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

/// Get available disk space on the Data volume in bytes.
fn get_disk_free() -> u64 {
    unsafe {
        let mut stat: libc::statfs = std::mem::zeroed();
        let path = std::ffi::CString::new("/System/Volumes/Data").unwrap();
        if libc::statfs(path.as_ptr(), &mut stat) == 0 {
            stat.f_bavail as u64 * stat.f_bsize as u64
        } else {
            0
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobStatus {
    Pending,
    Running,
    Done,
    Failed(String),
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CleanupJob {
    pub id: usize,
    pub category: String,
    pub strategy: String,
    pub tier: CleanupTier,
    pub pre_size: u64,
    pub status: JobStatus,
    pub post_size: Option<u64>,       // actual size remaining after cleanup
    pub bytes_freed: u64,
    pub verification: Option<String>,
    pub paths: Vec<PathBuf>,          // paths targeted
    pub command: Option<String>,      // shell command (for Official tier)
    pub started_at: Option<Instant>,
    pub finished_at: Option<Instant>,
}

impl CleanupJob {
    pub fn elapsed_str(&self) -> String {
        match (self.started_at, self.finished_at) {
            (Some(s), Some(f)) => format!("{:.1}s", f.duration_since(s).as_secs_f64()),
            (Some(s), None) => format!("{:.1}s...", s.elapsed().as_secs_f64()),
            _ => String::new(),
        }
    }

    pub fn status_str(&self) -> &str {
        match &self.status {
            JobStatus::Pending => "pending",
            JobStatus::Running => "running",
            JobStatus::Done => "done",
            JobStatus::Failed(_) => "FAILED",
        }
    }
}

/// Thread-safe job queue.
pub struct CleanupQueue {
    jobs: Arc<Mutex<Vec<CleanupJob>>>,
    next_id: Arc<Mutex<usize>>,
}

#[allow(dead_code)]
impl CleanupQueue {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(Mutex::new(Vec::new())),
            next_id: Arc::new(Mutex::new(1)),
        }
    }

    /// Add a job to the queue. It stays Pending until execute_all() is called.
    pub fn enqueue(&self, category: &str, action: CleanupAction) -> usize {
        let id = {
            let mut nid = self.next_id.lock().unwrap();
            let id = *nid;
            *nid += 1;
            id
        };

        let job = CleanupJob {
            id,
            category: category.to_string(),
            strategy: action.description.clone(),
            tier: action.tier,
            pre_size: action.estimated_savings,
            status: JobStatus::Pending,
            post_size: None,
            bytes_freed: 0,
            verification: None,
            paths: action.paths_to_remove.clone(),
            command: action.command.clone(),
            started_at: None,
            finished_at: None,
        };

        self.jobs.lock().unwrap().push(job);
        id
    }

    /// Remove a pending job from the queue.
    pub fn remove(&self, id: usize) {
        let mut jobs = self.jobs.lock().unwrap();
        jobs.retain(|j| j.id != id || j.status != JobStatus::Pending);
    }

    /// Execute ALL pending jobs in background threads.
    pub fn execute_all(&self) {
        let jobs_arc = self.jobs.clone();
        let pending_ids: Vec<usize> = {
            let jobs = jobs_arc.lock().unwrap();
            jobs.iter().filter(|j| j.status == JobStatus::Pending).map(|j| j.id).collect()
        };

        for job_id in pending_ids {
            let jobs = jobs_arc.clone();

            // Get the action details from the job
            let (paths, command, tier, pre_size) = {
                let mut jl = jobs.lock().unwrap();
                let j = match jl.iter_mut().find(|j| j.id == job_id) {
                    Some(j) => j,
                    None => continue,
                };
                j.status = JobStatus::Running;
                j.started_at = Some(Instant::now());
                (j.paths.clone(), j.command.clone(), j.tier, j.pre_size)
            };

            let action = CleanupAction {
                tier,
                description: String::new(),
                command,
                paths_to_remove: paths.clone(),
                estimated_savings: pre_size,
            };

            thread::spawn(move || {
                // Measure actual disk free space BEFORE
                let free_before = get_disk_free();

                let (_bytes_freed, error) = cleanup::execute_cleanup(&action);
                let verify = cleanup::verify_cleanup(&action);

                // Measure actual disk free space AFTER
                let free_after = get_disk_free();
                let actual_freed = free_after.saturating_sub(free_before);

                // Also measure what's left at the paths
                let post_size: u64 = paths.iter()
                    .map(|p| {
                        if p.is_dir() {
                            crate::util::dir_size(p)
                        } else {
                            std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
                        }
                    })
                    .sum();

                // Build honest verification message
                let verify_msg = if actual_freed == 0 && post_size > 0 {
                    format!("NO disk space freed — files may be locked/in use ({}  remaining)",
                        bytesize::ByteSize(post_size))
                } else if actual_freed == 0 {
                    format!("{} — but no disk space change detected", verify.summary)
                } else {
                    format!("{} — {} actually freed on disk", verify.summary,
                        bytesize::ByteSize(actual_freed))
                };

                let mut jl = jobs.lock().unwrap();
                if let Some(j) = jl.iter_mut().find(|j| j.id == job_id) {
                    j.finished_at = Some(Instant::now());
                    j.bytes_freed = actual_freed;
                    j.post_size = Some(post_size);
                    j.verification = Some(verify_msg);
                    j.status = if let Some(err) = error {
                        JobStatus::Failed(err)
                    } else if actual_freed == 0 && pre_size > 1_000_000 {
                        JobStatus::Failed("No disk space freed — may need sudo".into())
                    } else {
                        JobStatus::Done
                    };
                }
            });
        }
    }

    /// Get a snapshot of all jobs for display.
    pub fn snapshot(&self) -> Vec<CleanupJob> {
        self.jobs.lock().unwrap().clone()
    }

    /// Count of active (pending + running) jobs.
    pub fn active_count(&self) -> usize {
        self.jobs.lock().unwrap().iter()
            .filter(|j| matches!(j.status, JobStatus::Pending | JobStatus::Running))
            .count()
    }

    /// Total bytes freed across all completed jobs.
    pub fn total_freed(&self) -> u64 {
        self.jobs.lock().unwrap().iter()
            .filter(|j| j.status == JobStatus::Done)
            .map(|j| j.bytes_freed)
            .sum()
    }
}
