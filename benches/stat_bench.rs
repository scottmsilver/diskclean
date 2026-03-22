//! I/O benchmark: compare stat approaches to find the fastest way to get file metadata.
//! cargo build --release --bench stat_bench && ./target/release/deps/stat_bench-*

use std::fs;
use std::os::unix::fs::MetadataExt;
#[cfg(target_os = "macos")]
use std::os::macos::fs::MetadataExt as DarwinMetadataExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

const SF_DATALESS: u32 = 0x40000000;

fn bench_root() -> PathBuf {
    PathBuf::from(
        std::env::args().nth(1).unwrap_or_else(|| {
            format!("{}/Library/Caches", std::env::var("HOME").unwrap_or("/Users/ssilver".into()))
        })
    )
}

// Method 1: readdir + stat per file (current approach)
fn walk_readdir_stat(path: &Path) -> (u64, u64) {
    let mut bytes: u64 = 0;
    let mut count: u64 = 0;
    fn inner(path: &Path, bytes: &mut u64, count: &mut u64) {
        let entries = match fs::read_dir(path) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            *count += 1;
            if meta.is_dir() {
                inner(&entry.path(), bytes, count);
            } else {
                let flags = meta.st_flags() as u32;
                if flags & SF_DATALESS != 0 { continue; }
                *bytes += (meta.blocks() as u64) * 512;
            }
        }
    }
    inner(path, &mut bytes, &mut count);
    (bytes, count)
}

// Method 2: getattrlistbulk — batch metadata per directory in one syscall
fn walk_getattrlistbulk(path: &Path) -> (u64, u64) {
    let mut bytes: u64 = 0;
    let mut count: u64 = 0;

    fn inner(path: &Path, bytes: &mut u64, count: &mut u64) {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let c_path = match CString::new(path.as_os_str().as_bytes()) {
            Ok(p) => p,
            Err(_) => return,
        };

        // Open directory
        let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY) };
        if fd < 0 { return; }

        // Set up attrlist requesting: name, obj_type, data_size, physical_size, mod_time, flags
        #[repr(C)]
        #[derive(Default)]
        struct AttrList {
            bitmapcount: u16,
            reserved: u16,
            commonattr: u32,
            volattr: u32,
            dirattr: u32,
            fileattr: u32,
            forkattr: u32,
        }

        // ATTR_CMN_RETURNED_ATTRS = 0x80000000
        // ATTR_CMN_NAME = 0x00000001
        // ATTR_CMN_OBJTYPE = 0x00000008
        // ATTR_CMN_MODTIME = 0x00000400
        // ATTR_CMN_FLAGS = 0x00040000
        // ATTR_FILE_DATALENGTH = 0x00000002
        // ATTR_FILE_TOTALSIZE = 0x00000004  (physical)
        // ATTR_FILE_ALLOCSIZE = 0x00000004 (in file attrs)

        let mut alist = AttrList::default();
        alist.bitmapcount = 5; // ATTR_BIT_MAP_COUNT
        alist.commonattr = 0x80000000 | 0x00000001 | 0x00000008 | 0x00000400 | 0x00040000;
        alist.fileattr = 0x00000002; // ATTR_FILE_DATALENGTH

        // Buffer for results (256KB should handle most directories)
        let buf_size: usize = 256 * 1024;
        let mut buf: Vec<u8> = vec![0u8; buf_size];

        // FSOPT_NOFOLLOW = 0x00000001, FSOPT_PACK_INVAL_ATTRS = 0x00000008
        let options: u64 = 0x00000001 | 0x00000008;

        loop {
            let ret = unsafe {
                getattrlistbulk(
                    fd,
                    &alist as *const AttrList as *const libc::c_void,
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf_size,
                    options,
                )
            };

            if ret <= 0 { break; } // 0 = end of directory, -1 = error

            // Parse entries from buffer
            let mut offset: usize = 0;
            for _ in 0..ret {
                if offset + 4 > buf_size { break; }

                // First 4 bytes = length of this entry
                let entry_len = u32::from_ne_bytes([
                    buf[offset], buf[offset+1], buf[offset+2], buf[offset+3]
                ]) as usize;

                if entry_len == 0 || offset + entry_len > buf_size { break; }

                *count += 1;

                // Parse returned_attrs (attrgroup_t * 5 + 2 reserved = 24 bytes)
                // Then: name (attrreference_t = offset:4 + length:4)
                // Then: obj_type (u32: VREG=1, VDIR=2, VLNK=5)
                // Then: modtime (timespec = sec:8 + nsec:8 = 16 bytes)
                // Then: flags (u32)
                // Then: data_length (u64) — only for files

                let base = offset + 4; // skip entry length
                // returned_attrs: 24 bytes (bitmapcount:2 + reserved:2 + 5*4 attrs)
                let after_returned = base + 24;
                // name: attrreference_t (8 bytes: offset:4 + length:4)
                let after_name = after_returned + 8;
                // obj_type: 4 bytes
                let obj_type = if after_name + 4 <= offset + entry_len {
                    u32::from_ne_bytes([
                        buf[after_name], buf[after_name+1],
                        buf[after_name+2], buf[after_name+3]
                    ])
                } else { 0 };
                let after_objtype = after_name + 4;
                // modtime: timespec (16 bytes)
                let after_modtime = after_objtype + 16;
                // flags: u32
                let flags = if after_modtime + 4 <= offset + entry_len {
                    u32::from_ne_bytes([
                        buf[after_modtime], buf[after_modtime+1],
                        buf[after_modtime+2], buf[after_modtime+3]
                    ])
                } else { 0 };
                let after_flags = after_modtime + 4;

                if obj_type == 2 {
                    // Directory — recurse
                    // Get name from attrreference
                    let name_off_val = u32::from_ne_bytes([
                        buf[after_returned], buf[after_returned+1],
                        buf[after_returned+2], buf[after_returned+3]
                    ]) as usize;
                    let name_ptr = after_returned + name_off_val;
                    if name_ptr < offset + entry_len {
                        // Read null-terminated string
                        let mut end = name_ptr;
                        while end < offset + entry_len && buf[end] != 0 { end += 1; }
                        if let Ok(name) = std::str::from_utf8(&buf[name_ptr..end]) {
                            if name != "." && name != ".." {
                                let child = path.join(name);
                                inner(&child, bytes, count);
                            }
                        }
                    }
                } else if obj_type == 1 {
                    // Regular file
                    if flags & SF_DATALESS != 0 {
                        offset += entry_len;
                        continue;
                    }
                    // data_length: u64
                    if after_flags + 8 <= offset + entry_len {
                        let data_len = u64::from_ne_bytes([
                            buf[after_flags], buf[after_flags+1], buf[after_flags+2], buf[after_flags+3],
                            buf[after_flags+4], buf[after_flags+5], buf[after_flags+6], buf[after_flags+7],
                        ]);
                        *bytes += data_len;
                    }
                }

                offset += entry_len;
            }
        }

        unsafe { libc::close(fd); }
    }

    inner(path, &mut bytes, &mut count);
    (bytes, count)
}

extern "C" {
    fn getattrlistbulk(
        dirfd: libc::c_int,
        alist: *const libc::c_void,
        attributeBuffer: *mut libc::c_void,
        bufferSize: libc::size_t,
        options: u64,
    ) -> libc::c_int;
}

// Method 3: readdir only (no stat — floor for traversal)
fn walk_readdir_only(path: &Path) -> (u64, u64) {
    let mut count: u64 = 0;
    fn inner(path: &Path, count: &mut u64) {
        let entries = match fs::read_dir(path) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            *count += 1;
            if let Ok(ft) = entry.file_type() {
                if ft.is_dir() { inner(&entry.path(), count); }
            }
        }
    }
    inner(path, &mut count);
    (0, count)
}

fn bench(name: &str, f: impl Fn(&Path) -> (u64, u64), path: &Path) {
    let _ = f(path); // warmup
    let start = Instant::now();
    let (bytes, count) = f(path);
    let elapsed = start.elapsed();
    eprintln!(
        "  {:<30} {:>7.0}ms  {:>8} entries  {}",
        name,
        elapsed.as_secs_f64() * 1000.0,
        count,
        bytesize::ByteSize(bytes),
    );
}

fn main() {
    let root = bench_root();
    eprintln!("stat benchmark: {:?}", root);
    eprintln!();
    bench("readdir + stat (current)", walk_readdir_stat, &root);
    bench("getattrlistbulk (batched)", walk_getattrlistbulk, &root);
    bench("readdir only (no stat)", walk_readdir_only, &root);
}
