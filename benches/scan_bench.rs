//! Benchmark: scan a directory tree. Compares jwalk vs getattrlistbulk.
//! Usage: scan_bench [iters] [path|full] [bulk]

use crossbeam_channel::unbounded;
use std::path::PathBuf;
use std::time::Instant;

use diskclean::model::ScanEvent;
use diskclean::scanner::walk;

fn drain(rx: crossbeam_channel::Receiver<ScanEvent>) -> (u64, u64, u64) {
    let mut findings = 0u64;
    let mut total_size = 0u64;
    let mut file_count = 0u64;
    loop {
        match rx.recv() {
            Ok(ScanEvent::Found(_, f)) => { findings += 1; total_size += f.physical_size; }
            Ok(ScanEvent::Complete(s)) => { file_count = s.files_scanned; break; }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    (findings, total_size, file_count)
}

fn main() {
    let iters: u32 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(1);
    let path_arg = std::env::args().nth(2).unwrap_or_else(|| {
        format!("{}/development", std::env::var("HOME").unwrap_or("/Users/ssilver".into()))
    });
    let use_bulk = std::env::args().any(|a| a == "bulk");
    let use_full = path_arg == "full";
    let root = PathBuf::from(&path_arg);

    let mode = if use_bulk { "BULK (getattrlistbulk)" } else { "JWALK (readdir+stat)" };
    if use_full {
        eprintln!("bench: FULL SCAN [{}], {} iters", mode, iters);
    } else {
        eprintln!("bench: {:?} [{}], {} iters", root, mode, iters);
    }

    let mut times = Vec::new();
    for i in 0..iters {
        let (tx, rx) = unbounded();
        let start = Instant::now();

        if use_bulk {
            if use_full {
                for home in walk::get_user_homes() {
                    let (htx, hrx) = unbounded::<ScanEvent>();
                    walk::run_scan_bulk(&home, htx);
                    // Forward findings to main channel
                    loop {
                        match hrx.recv() {
                            Ok(ScanEvent::Found(c, f)) => { let _ = tx.send(ScanEvent::Found(c, f)); }
                            Ok(ScanEvent::Complete(_)) => break,
                            Ok(e) => { let _ = tx.send(e); }
                            Err(_) => break,
                        }
                    }
                }
                let _ = tx.send(ScanEvent::Complete(diskclean::model::ScanResult {
                    categories: vec![], grand_total: 0, safe_total: 0, cloud_total: 0,
                    files_scanned: 0, perm_errors: 0, dataless_skipped: 0,
                    elapsed: start.elapsed(),
                }));
            } else {
                walk::run_scan_bulk(&root, tx);
            }
        } else if use_full {
            walk::run_scan(tx);
        } else {
            walk::run_scan_path(&root, tx);
        }

        let (findings, total_size, file_count) = drain(rx);
        let elapsed = start.elapsed();
        if i == 0 {
            eprintln!("  {} findings, {}, {} files", findings, bytesize::ByteSize(total_size), file_count);
        }
        eprintln!("  iter {}: {:.0}ms", i + 1, elapsed.as_secs_f64() * 1000.0);
        times.push(elapsed);
    }

    times.sort();
    if !times.is_empty() {
        eprintln!("  min={:.0}ms  median={:.0}ms", times[0].as_secs_f64() * 1000.0, times[times.len()/2].as_secs_f64() * 1000.0);
    }
}
