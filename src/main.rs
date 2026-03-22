mod model;
mod scanner;
mod staging;
mod tui;
mod util;

use bytesize::ByteSize;
use model::*;
use users::os::unix::UserExt;
use std::collections::BTreeMap;
use std::env;
use std::os::unix::process::CommandExt;
use std::process::Command;
use util::{format_age, username_from_uid};

fn ensure_root() {
    // Already root — nothing to do
    if unsafe { libc::geteuid() } == 0 {
        return;
    }

    // Save calling user info so the staging dir is owned by them
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };

    let exe = env::current_exe().expect("cannot determine own path");
    let args: Vec<String> = env::args().skip(1).collect();

    eprintln!("diskclean needs root for full scanning. Requesting sudo...");

    let mut cmd = Command::new("sudo");
    cmd.arg("--preserve-env=DISKCLEAN_CALLER_UID,DISKCLEAN_CALLER_GID");
    cmd.env("DISKCLEAN_CALLER_UID", uid.to_string());
    cmd.env("DISKCLEAN_CALLER_GID", gid.to_string());
    cmd.arg(&exe);
    cmd.args(&args);

    // exec replaces this process
    let err = cmd.exec();
    eprintln!("Failed to exec sudo: {}", err);
    std::process::exit(1);
}

/// Get the original (non-root) user's uid/gid and home dir for staging
pub fn caller_info() -> (u32, u32, std::path::PathBuf) {
    let uid: u32 = env::var("DISKCLEAN_CALLER_UID")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| unsafe { libc::getuid() });

    let gid: u32 = env::var("DISKCLEAN_CALLER_GID")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| unsafe { libc::getgid() });

    // Resolve home dir from uid
    let home = users::get_user_by_uid(uid)
        .map(|u| std::path::PathBuf::from(u.home_dir()))
        .unwrap_or_else(|| {
            env::var("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"))
        });

    (uid, gid, home)
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let plain_mode = args.iter().any(|a| a == "--plain" || a == "--scan")
        || !atty::is(atty::Stream::Stdout);
    let no_sudo = args.iter().any(|a| a == "--no-sudo");

    if !no_sudo {
        ensure_root();
    }

    if plain_mode {
        run_plain();
    } else if let Err(e) = tui::run_tui() {
        eprintln!("TUI error: {}. Try --plain for text output.", e);
        std::process::exit(1);
    }
}

fn run_plain() {
    println!("╔══════════════════════════════════════════════════════════════════╗");
    println!("║             DISKCLEAN — Full Semantic Disk Analyzer             ║");
    println!("╚══════════════════════════════════════════════════════════════════╝");
    println!();

    let (_handle, scan_rx) = scanner::spawn_scan();
    let mut cats: BTreeMap<Category, Vec<Finding>> = BTreeMap::new();
    let mut last_phase = String::new();
    let mut final_result: Option<ScanResult> = None;

    loop {
        match scan_rx.recv() {
            Ok(ScanEvent::Progress(p)) => {
                let phase = match &p.phase {
                    ScanPhase::DetectingApps => "Detecting apps...".to_string(),
                    ScanPhase::ScanningUser(u) => format!("Scanning /Users/{}...", u),
                    ScanPhase::ScanningSystem => "Scanning system dirs...".to_string(),
                    ScanPhase::Complete => "Complete".to_string(),
                };
                if phase != last_phase {
                    eprintln!("  ⟳ {}", phase);
                    last_phase = phase;
                }
            }
            Ok(ScanEvent::Found(cat, finding)) => {
                cats.entry(cat).or_default().push(finding);
            }
            Ok(ScanEvent::Complete(result)) => {
                final_result = Some(result);
                break;
            }
            Err(_) => break,
        }
    }

    let mut sorted: Vec<(Category, u64, Vec<Finding>)> = Vec::new();
    for (cat, mut findings) in cats {
        let total: u64 = findings.iter().map(|f| f.physical_size).sum();
        findings.sort_by(|a, b| b.physical_size.cmp(&a.physical_size));
        sorted.push((cat, total, findings));
    }
    sorted.sort_by(|a, b| b.1.cmp(&a.1));

    let grand_total: u64 = sorted.iter().map(|(_, s, _)| s).sum();
    let safe_total: u64 = sorted.iter()
        .filter(|(c, _, _)| c.risk_level() == RiskLevel::Safe)
        .map(|(_, s, _)| s).sum();

    let stats = final_result.as_ref();
    let elapsed = stats.map(|s| s.elapsed.as_secs_f64()).unwrap_or(0.0);
    let files = stats.map(|s| s.files_scanned).unwrap_or(0);
    let dataless = stats.map(|s| s.dataless_skipped).unwrap_or(0);
    let errors = stats.map(|s| s.perm_errors).unwrap_or(0);

    println!();
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!(" Total reclaimable: {} (physical)  |  {} files in {:.1}s  |  {} iCloud-evicted",
        ByteSize(grand_total), files, elapsed, dataless);
    if errors > 0 {
        println!(" Permission errors: {} (run with sudo for full scan)", errors);
    }
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    for (cat, total_size, items) in &sorted {
        if *total_size == 0 { continue; }
        let risk = cat.risk_level();
        println!("┌─ {} ({}) ── {} ─", cat.label(), ByteSize(*total_size), risk.label());
        println!("│");
        println!("│  {}", cat.advice());
        println!("│");

        let show = items.len().min(10);
        for f in &items[..show] {
            let user = username_from_uid(f.owner_uid);
            let age = format_age(f.last_modified);
            let cloud = if f.cloud_backed { " ☁" } else { "" };
            println!("│  {:>10} [{}] {} ({}){}", ByteSize(f.physical_size), user, f.path.display(), age, cloud);
            if !f.detail.is_empty() {
                println!("│  {:>10} {}", "", f.detail);
            }
        }
        if items.len() > show {
            let hidden: u64 = items[show..].iter().map(|f| f.physical_size).sum();
            println!("│  ... {} more ({})", items.len() - show, ByteSize(hidden));
        }
        println!("│");
        println!("└──────────────────────────────────────────────────────────────");
        println!();
    }

    println!("══════════════════════════════════════════════════════════════════");
    println!(" Safe to delete: {}  |  Total: {}", ByteSize(safe_total), ByteSize(grand_total));
    println!("══════════════════════════════════════════════════════════════════");
    println!();
    println!("  All sizes are physical (on-disk, APFS-aware).");
    println!("  Nothing was deleted — this is read-only.");
    println!();
}
