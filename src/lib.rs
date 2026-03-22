pub mod cleanup;
pub mod model;
pub mod safety_oracle;
pub mod scanner;
pub mod staging;
pub mod tui;
pub mod util;

pub use crate::staging::StagingDir;

/// Get the original (non-root) user's uid/gid and home dir for staging
pub fn caller_info() -> (u32, u32, std::path::PathBuf) {
    let uid: u32 = std::env::var("DISKCLEAN_CALLER_UID")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| unsafe { libc::getuid() });

    let gid: u32 = std::env::var("DISKCLEAN_CALLER_GID")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| unsafe { libc::getgid() });

    use users::os::unix::UserExt;
    let home = users::get_user_by_uid(uid)
        .map(|u| std::path::PathBuf::from(u.home_dir()))
        .unwrap_or_else(|| {
            std::env::var("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"))
        });

    (uid, gid, home)
}
