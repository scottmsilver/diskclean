use std::path::PathBuf;
use std::time::SystemTime;

pub struct Finding {
    pub path: PathBuf,
    pub physical_size: u64,
    pub logical_size: u64,
    pub last_modified: Option<SystemTime>,
    pub owner_uid: u32,
    pub cloud_backed: bool,
    pub detail: String,
}
