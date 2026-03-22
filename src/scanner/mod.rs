pub mod bulkwalk;
pub mod classify;
pub mod walk;

use crate::model::ScanEvent;
use crossbeam_channel::{Receiver, unbounded};
use std::thread::{self, JoinHandle};

pub fn spawn_scan() -> (JoinHandle<()>, Receiver<ScanEvent>) {
    let (tx, rx) = unbounded();
    let handle = thread::spawn(move || {
        walk::run_scan(tx);
    });
    (handle, rx)
}
