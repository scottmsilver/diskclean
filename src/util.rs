use std::time::SystemTime;

pub fn format_age(sys_time: Option<SystemTime>) -> String {
    sys_time
        .and_then(|t| t.elapsed().ok())
        .map(|d| {
            let days = d.as_secs() / 86400;
            if days > 365 { format!("{}y ago", days / 365) }
            else if days > 30 { format!("{}mo ago", days / 30) }
            else if days > 0 { format!("{}d ago", days) }
            else { "today".to_string() }
        })
        .unwrap_or_else(|| "unknown".to_string())
}

pub fn username_from_uid(uid: u32) -> String {
    users::get_user_by_uid(uid)
        .map(|u| u.name().to_string_lossy().to_string())
        .unwrap_or_else(|| format!("uid:{}", uid))
}

pub fn is_media_ext(ext: &str) -> bool {
    matches!(ext,
        "mp4" | "mov" | "avi" | "mkv" | "wmv" | "flv" | "webm" | "m4v" | "mpg" | "mpeg"
        | "mp3" | "wav" | "flac" | "aac" | "ogg" | "wma" | "m4a" | "aiff"
        | "psd" | "tiff" | "tif" | "raw" | "cr2" | "nef" | "arw" | "dng" | "bmp"
        | "dmg" | "iso"
    )
}

pub fn is_vm_ext(ext: &str) -> bool {
    matches!(ext, "vmdk" | "vdi" | "qcow2" | "vhd" | "vhdx" | "pvm" | "hdd")
}

pub fn dir_size(path: &std::path::Path) -> u64 {
    let mut total: u64 = 0;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_dir() {
                    total += dir_size(&entry.path());
                } else {
                    total += meta.len();
                }
            }
        }
    }
    total
}
