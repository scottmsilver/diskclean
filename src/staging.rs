use std::fs;
use std::path::{Path, PathBuf};

/// The staging directory where items are moved before final deletion.
/// Created under the calling user's home dir, owned by that user.
pub struct StagingDir {
    pub path: PathBuf,
    caller_uid: u32,
    caller_gid: u32,
}

impl StagingDir {
    pub fn new() -> Self {
        let (uid, gid, home) = crate::caller_info();
        let path = home.join("To Delete");
        Self {
            path,
            caller_uid: uid,
            caller_gid: gid,
        }
    }

    /// Ensure the staging directory exists and is owned by the caller.
    pub fn ensure_exists(&self) -> Result<(), String> {
        if !self.path.exists() {
            fs::create_dir_all(&self.path)
                .map_err(|e| format!("Failed to create {:?}: {}", self.path, e))?;
        }
        self.chown(&self.path)?;
        Ok(())
    }

    /// Move a path into the staging directory, preserving its name.
    /// If a name collision exists, appends a numeric suffix.
    /// Returns the destination path.
    pub fn stage(&self, source: &Path) -> Result<PathBuf, String> {
        self.ensure_exists()?;

        let name = source.file_name()
            .ok_or_else(|| format!("Cannot determine name for {:?}", source))?;

        let mut dest = self.path.join(name);

        // Handle name collisions
        if dest.exists() {
            let stem = dest.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("item")
                .to_string();
            let ext = dest.extension()
                .and_then(|e| e.to_str())
                .map(|e| format!(".{}", e))
                .unwrap_or_default();

            for i in 2..1000 {
                dest = self.path.join(format!("{} {}{}", stem, i, ext));
                if !dest.exists() { break; }
            }
        }

        // Try rename first (instant if same volume)
        match fs::rename(source, &dest) {
            Ok(()) => {
                // chown the moved item to the calling user
                self.chown_recursive(&dest)?;
                return Ok(dest);
            }
            Err(_) => {
                // Cross-volume: fall back to copy + delete
                // For directories, use /bin/mv which handles this
                let status = std::process::Command::new("/bin/mv")
                    .arg(source)
                    .arg(&dest)
                    .status()
                    .map_err(|e| format!("mv failed: {}", e))?;

                if !status.success() {
                    return Err(format!("mv {:?} -> {:?} exited with {}", source, dest, status));
                }

                self.chown_recursive(&dest)?;
                Ok(dest)
            }
        }
    }

    /// chown a single path to the calling user
    fn chown(&self, path: &Path) -> Result<(), String> {
        let ret = unsafe {
            libc::chown(
                std::ffi::CString::new(path.to_string_lossy().as_bytes())
                    .map_err(|e| format!("CString error: {}", e))?
                    .as_ptr(),
                self.caller_uid,
                self.caller_gid,
            )
        };
        if ret != 0 {
            // Non-fatal: log but continue
            let _ = ret;
        }
        Ok(())
    }

    /// Recursively chown to calling user using /usr/sbin/chown -R
    fn chown_recursive(&self, path: &Path) -> Result<(), String> {
        let owner = format!("{}:{}", self.caller_uid, self.caller_gid);
        let _ = std::process::Command::new("/usr/sbin/chown")
            .args(["-R", &owner])
            .arg(path)
            .status();
        Ok(())
    }

    /// How many items are currently staged
    pub fn staged_count(&self) -> usize {
        if !self.path.exists() { return 0; }
        fs::read_dir(&self.path).map(|e| e.count()).unwrap_or(0)
    }

    /// Total size of staged items
    pub fn staged_size(&self) -> u64 {
        if !self.path.exists() { return 0; }
        crate::util::dir_size(&self.path)
    }
}
