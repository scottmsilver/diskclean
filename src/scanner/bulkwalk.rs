//! Fast directory walker using macOS getattrlistbulk().
//! Returns directory entries with full metadata (name, type, size, flags, mtime)
//! in a single syscall per directory — 4-5x faster than readdir + stat.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SF_DATALESS: u32 = 0x40000000;

// macOS constants
const ATTR_BIT_MAP_COUNT: u16 = 5;
const ATTR_CMN_RETURNED_ATTRS: u32 = 0x80000000;
const ATTR_CMN_NAME: u32 = 0x00000001;
const ATTR_CMN_OBJTYPE: u32 = 0x00000008;
const ATTR_CMN_MODTIME: u32 = 0x00000400;
const ATTR_CMN_FLAGS: u32 = 0x00040000;
const ATTR_FILE_DATALENGTH: u32 = 0x00000002;
const ATTR_FILE_ALLOCSIZE: u32 = 0x00000004;
const FSOPT_NOFOLLOW: u64 = 0x00000001;
const FSOPT_PACK_INVAL_ATTRS: u64 = 0x00000008;
const VDIR: u32 = 2;
const VREG: u32 = 1;

#[repr(C)]
struct AttrList {
    bitmapcount: u16,
    reserved: u16,
    commonattr: u32,
    volattr: u32,
    dirattr: u32,
    fileattr: u32,
    forkattr: u32,
}

extern "C" {
    fn getattrlistbulk(
        dirfd: libc::c_int,
        alist: *const AttrList,
        attributeBuffer: *mut libc::c_void,
        bufferSize: libc::size_t,
        options: u64,
    ) -> libc::c_int;
}

/// Metadata for a single directory entry, returned by getattrlistbulk.
pub struct EntryInfo {
    pub name: String,
    pub is_dir: bool,
    pub is_file: bool,
    pub is_dataless: bool,
    pub logical_size: u64,
    pub physical_size: u64,
    pub modified: Option<SystemTime>,
    pub flags: u32,
}

/// Walk a directory non-recursively using getattrlistbulk.
/// Returns all entries with full metadata in one pass.
pub fn list_dir_bulk(path: &Path) -> Vec<EntryInfo> {
    let c_path = match CString::new(path.as_os_str().as_bytes()) {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY) };
    if fd < 0 { return Vec::new(); }
    let result = list_dir_bulk_fd(fd);
    unsafe { libc::close(fd); }
    result
}

/// Open a subdirectory relative to a parent fd (avoids full path resolution).
fn open_subdir(parent_fd: libc::c_int, name: &str) -> libc::c_int {
    let c_name = match CString::new(name.as_bytes()) {
        Ok(n) => n,
        Err(_) => return -1,
    };
    unsafe { libc::openat(parent_fd, c_name.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY) }
}

/// Read directory entries from an already-open fd.
/// Uses a thread-local reusable buffer to avoid 1MB allocation per call.
fn list_dir_bulk_fd(fd: libc::c_int) -> Vec<EntryInfo> {
    let mut results = Vec::new();

    thread_local! {
        static BUF: std::cell::RefCell<Vec<u8>> = std::cell::RefCell::new(vec![0u8; 1024 * 1024]);
    }

    let alist = AttrList {
        bitmapcount: ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: ATTR_CMN_RETURNED_ATTRS | ATTR_CMN_NAME | ATTR_CMN_OBJTYPE
            | ATTR_CMN_MODTIME | ATTR_CMN_FLAGS,
        volattr: 0,
        dirattr: 0,
        fileattr: ATTR_FILE_DATALENGTH | ATTR_FILE_ALLOCSIZE,
        forkattr: 0,
    };

    let options: u64 = FSOPT_NOFOLLOW | FSOPT_PACK_INVAL_ATTRS;

    BUF.with(|buf_cell| {
    let mut buf = buf_cell.borrow_mut();
    let buf_size = buf.len();

    loop {
        let count = unsafe {
            getattrlistbulk(
                fd,
                &alist,
                buf.as_mut_ptr() as *mut libc::c_void,
                buf_size,
                options,
            )
        };

        if count <= 0 { break; }

        let mut offset: usize = 0;
        for _ in 0..count {
            if offset + 4 > buf_size { break; }

            let entry_len = read_u32(&buf, offset) as usize;
            if entry_len == 0 || offset + entry_len > buf_size { break; }

            // Layout (variable — file attrs only present for regular files):
            //   4: length
            //   24: returned_attrs (attribute_set_t: bitmapcount(2)+reserved(2)+5*u32)
            //   8: name (attrreference_t: data_offset(4) + data_length(4))
            //   4: obj_type
            //   16: mod_time (timespec: tv_sec(8) + tv_nsec(8))
            //   4: flags
            //   [if file] 8: data_length (off_t)
            //   [if file] 8: alloc_size (off_t)
            //   name data (null-terminated string)

            let base = offset + 4;

            // returned_attrs = attribute_set_t: common(4) + vol(4) + dir(4) + file(4) + fork(4) = 20 bytes
            // fileattr is at offset 12 within attribute_set_t (after common+vol+dir)
            let returned_fileattr = read_u32(&buf, base + 12);

            let name_ref_offset = base + 20; // attribute_set_t is 20 bytes

            // Parse name via attrreference_t
            let name_data_off = read_u32(&buf, name_ref_offset) as usize;
            let name_start = name_ref_offset + name_data_off;
            let name = read_cstring(&buf, name_start, offset + entry_len);

            // obj_type
            let obj_type_offset = name_ref_offset + 8;
            let obj_type = read_u32(&buf, obj_type_offset);

            // mod_time (timespec)
            let modtime_offset = obj_type_offset + 4;
            let tv_sec = read_i64(&buf, modtime_offset);
            let modified = if tv_sec > 0 {
                UNIX_EPOCH.checked_add(Duration::from_secs(tv_sec as u64))
            } else {
                None
            };

            // flags
            let flags_offset = modtime_offset + 16;
            let flags = read_u32(&buf, flags_offset);

            let is_dataless = flags & SF_DATALESS != 0;

            // file sizes — only present if returned_fileattr has the bits set
            let has_file_sizes = returned_fileattr != 0;
            let (logical_size, physical_size) = if has_file_sizes && obj_type == VREG {
                let data_len_offset = flags_offset + 4;
                let alloc_size_offset = data_len_offset + 8;
                let logical = read_u64(&buf, data_len_offset);
                let physical = read_u64(&buf, alloc_size_offset);
                (logical, physical)
            } else {
                (0, 0)
            };

            results.push(EntryInfo {
                name,
                is_dir: obj_type == VDIR,
                is_file: obj_type == VREG,
                is_dataless,
                logical_size,
                physical_size,
                modified,
                flags,
            });

            offset += entry_len;
        }
    }

    }); // BUF.with

    results
}

/// Recursively walk a directory tree using getattrlistbulk.
/// Calls the visitor for every entry with its full path and metadata.
/// The visitor returns false to skip descending into a directory.
pub fn walk_bulk<F>(root: &Path, mut visitor: F)
where
    F: FnMut(&Path, &EntryInfo) -> bool,
{
    walk_bulk_inner(root, &mut visitor);
}

fn walk_bulk_inner<F>(dir: &Path, visitor: &mut F)
where
    F: FnMut(&Path, &EntryInfo) -> bool,
{
    let entries = list_dir_bulk(dir);
    let mut subdirs: Vec<PathBuf> = Vec::new();

    for entry in &entries {
        let child_path = dir.join(&entry.name);
        let descend = visitor(&child_path, entry);
        if entry.is_dir && descend {
            subdirs.push(child_path);
        }
    }

    for subdir in subdirs {
        walk_bulk_inner(&subdir, visitor);
    }
}

/// Fully parallel walk using rayon work-stealing at EVERY level.
/// Uses openat() for subdirectories to avoid path resolution overhead.
pub fn walk_bulk_parallel<F>(root: &Path, visitor: F)
where
    F: Fn(&Path, &EntryInfo) -> bool + Send + Sync,
{
    let c_root = match CString::new(root.as_os_str().as_bytes()) {
        Ok(p) => p,
        Err(_) => return,
    };
    let root_fd = unsafe { libc::open(c_root.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY) };
    if root_fd < 0 { return; }

    walk_parallel_fd(root, root_fd, &visitor);
    unsafe { libc::close(root_fd); }
}

fn walk_parallel_fd<F>(dir: &Path, dir_fd: libc::c_int, visitor: &F)
where
    F: Fn(&Path, &EntryInfo) -> bool + Send + Sync,
{
    use rayon::prelude::*;

    let entries = list_dir_bulk_fd(dir_fd);
    let subdirs: Vec<(PathBuf, String)> = entries.iter()
        .filter_map(|entry| {
            let child_path = dir.join(&entry.name);
            let descend = visitor(&child_path, entry);
            if entry.is_dir && descend {
                Some((child_path, entry.name.clone()))
            } else {
                None
            }
        })
        .collect();

    // Rayon work-stealing with openat for each subdir
    subdirs.par_iter().for_each(|(child_path, name)| {
        let child_fd = open_subdir(dir_fd, name);
        if child_fd >= 0 {
            walk_parallel_fd(child_path, child_fd, visitor);
            unsafe { libc::close(child_fd); }
        }
    });
}

// ── Buffer reading helpers ──────────────────────────────────────────────────

#[inline]
fn read_u32(buf: &[u8], offset: usize) -> u32 {
    if offset + 4 > buf.len() { return 0; }
    u32::from_ne_bytes([buf[offset], buf[offset+1], buf[offset+2], buf[offset+3]])
}

#[inline]
fn read_u64(buf: &[u8], offset: usize) -> u64 {
    if offset + 8 > buf.len() { return 0; }
    u64::from_ne_bytes([
        buf[offset], buf[offset+1], buf[offset+2], buf[offset+3],
        buf[offset+4], buf[offset+5], buf[offset+6], buf[offset+7],
    ])
}

#[inline]
fn read_i64(buf: &[u8], offset: usize) -> i64 {
    if offset + 8 > buf.len() { return 0; }
    i64::from_ne_bytes([
        buf[offset], buf[offset+1], buf[offset+2], buf[offset+3],
        buf[offset+4], buf[offset+5], buf[offset+6], buf[offset+7],
    ])
}

fn read_cstring(buf: &[u8], start: usize, limit: usize) -> String {
    let end = (start..limit.min(buf.len())).find(|&i| buf[i] == 0).unwrap_or(limit.min(buf.len()));
    String::from_utf8_lossy(&buf[start..end]).to_string()
}
