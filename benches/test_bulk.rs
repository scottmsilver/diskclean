use diskclean::scanner::bulkwalk;
use std::path::Path;

fn main() {
    let root = Path::new("/Users/ssilver/Library/Caches");

    // Test top level
    let entries = bulkwalk::list_dir_bulk(root);
    eprintln!("top-level: {} entries", entries.len());
    let mut dirs = 0;
    for (i, e) in entries.iter().enumerate() {
        if e.is_dir { dirs += 1; }
        if i < 5 {
            eprintln!("  [{}] name='{}' is_dir={} is_file={} flags={:#x} logical={} phys={}",
                i, e.name, e.is_dir, e.is_file, e.flags, e.logical_size, e.physical_size);
        }
    }
    eprintln!("  {} dirs", dirs);

    // Test recursion manually
    if let Some(d) = entries.iter().find(|e| e.is_dir) {
        let child = root.join(&d.name);
        eprintln!("\nrecurse into: {:?}", child);
        let child_entries = bulkwalk::list_dir_bulk(&child);
        eprintln!("  child has {} entries", child_entries.len());
    }

    // Test full walk
    let mut total = 0u64;
    let mut total_bytes = 0u64;
    bulkwalk::walk_bulk(root, |_path, entry| {
        total += 1;
        if entry.is_file {
            total_bytes += entry.physical_size;
        }
        true // always descend
    });
    eprintln!("\nwalk_bulk total: {} entries, {} bytes", total, total_bytes);
}
