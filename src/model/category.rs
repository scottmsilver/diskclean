use super::risk::RiskLevel;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    // Caches (always safe)
    PackageManagerCache,
    AppCache,
    BrowserCache,
    BuildArtifact,
    NodeModules,
    XcodeDerivedData,
    LogsAndDiagnostics,
    CrashReports,
    TmpFiles,
    ElectronCache,

    // Runtime/toolchain sprawl
    OldNodeVersions,
    PythonVenvs,
    CondaInstall,
    OldIdeExtensions,
    RustupToolchains,
    AndroidSdk,
    DartPubCache,
    CachedBrowserBinaries, // puppeteer, selenium, playwright chromium downloads

    // Stale / forgotten
    CloudSyncedLocal,
    StaleProject,
    OldDownloads,
    Trash,

    // Big items to review
    DockerData,
    IosDeviceBackup,
    VmImages,
    LargeMedia,
    LargeOther,
    OldAppLeftovers,
    SimulatorRuntimes,

    // System
    TimeMachineLocal,
    CoreDumps,
    MailAttachments,
    HomebrewOldVersions,
}

impl Category {
    pub fn label(&self) -> &str {
        match self {
            Self::PackageManagerCache => "Package Manager Caches",
            Self::AppCache => "Application Caches",
            Self::BrowserCache => "Browser Caches",
            Self::BuildArtifact => "Build Artifacts",
            Self::NodeModules => "node_modules Directories",
            Self::XcodeDerivedData => "Xcode DerivedData",
            Self::LogsAndDiagnostics => "Logs & Diagnostics",
            Self::CrashReports => "Crash Reports",
            Self::TmpFiles => "Temporary Files",
            Self::ElectronCache => "Electron App Caches",
            Self::OldNodeVersions => "Old Node.js Versions (nvm)",
            Self::PythonVenvs => "Python Virtual Environments",
            Self::CondaInstall => "Conda/Anaconda Installation",
            Self::OldIdeExtensions => "Old IDE Extensions (VS Code, Cursor, Windsurf)",
            Self::RustupToolchains => "Rust Toolchains (rustup)",
            Self::AndroidSdk => "Android SDK Components",
            Self::DartPubCache => "Dart/Flutter Pub Cache",
            Self::CachedBrowserBinaries => "Cached Browser Binaries (Puppeteer, Selenium)",
            Self::CloudSyncedLocal => "Cloud-Synced (local copy redundant)",
            Self::StaleProject => "Stale Dev Projects (>6 months)",
            Self::OldDownloads => "Old Downloads (>90 days)",
            Self::Trash => "Trash",
            Self::DockerData => "Docker Data",
            Self::IosDeviceBackup => "iOS Device Backups",
            Self::VmImages => "Virtual Machine Images",
            Self::LargeMedia => "Large Media (>200MB)",
            Self::LargeOther => "Other Large Files (>200MB)",
            Self::OldAppLeftovers => "Uninstalled App Leftovers",
            Self::SimulatorRuntimes => "Simulator Runtimes",
            Self::TimeMachineLocal => "Time Machine Local Snapshots",
            Self::CoreDumps => "Core Dumps",
            Self::MailAttachments => "Mail Attachments",
            Self::HomebrewOldVersions => "Homebrew Old Versions",
        }
    }

    pub fn advice(&self) -> &str {
        match self {
            Self::PackageManagerCache => "Cached packages (npm, pip, Homebrew, Cargo, etc). Completely safe — re-downloads on next install.",
            Self::AppCache => "App data caches. Safe to delete — apps rebuild on launch. May see brief slowdown.",
            Self::BrowserCache => "Cached web content. Safe — pages may load slower briefly.",
            Self::BuildArtifact => "Compiled code. Safe to delete — rebuilds automatically. Focus on projects you haven't touched.",
            Self::NodeModules => "JavaScript dependencies. Safe — 'npm install' restores. Prioritize old/inactive projects.",
            Self::XcodeDerivedData => "Xcode indexes and caches. Safe — Xcode regenerates everything on open.",
            Self::LogsAndDiagnostics => "System and app log files. Safe unless you're actively debugging something.",
            Self::CrashReports => "Crash logs and diagnostics. Safe unless debugging a specific crash.",
            Self::TmpFiles => "Temp files left by various processes. Safe for anything >1hr old.",
            Self::ElectronCache => "Cached Electron frameworks for apps like VS Code, Slack, Discord. Safe — apps re-download.",
            Self::OldNodeVersions => "Old Node.js versions managed by nvm. Keep the one you use daily, remove the rest. 'nvm ls' shows installed versions.",
            Self::PythonVenvs => "Python virtual environments inside projects. Each is a full copy of packages. Safe to delete — recreate with 'python -m venv .venv && pip install -r requirements.txt'. Target stale projects first.",
            Self::CondaInstall => "Anaconda/Miniconda installation including cached packages. The pkgs/ directory alone is often 5-10GB. Run 'conda clean --all' to reclaim cache, or remove entirely if you've switched to pip/uv.",
            Self::OldIdeExtensions => "Old versions of VS Code, Cursor, and Windsurf extensions. IDEs keep old versions after updates. Safe to remove — the current version is kept separately.",
            Self::RustupToolchains => "Rust compiler toolchains. Each target triple is ~1GB. Remove unused targets with 'rustup target remove <target>'. Keep your primary toolchain.",
            Self::AndroidSdk => "Android SDK build tools, emulator images, and platform files. Old API levels and unused system images can be removed via Android Studio's SDK Manager.",
            Self::DartPubCache => "Dart/Flutter package cache. Safe to delete — 'flutter pub get' re-downloads. Can grow large with many Flutter projects.",
            Self::CachedBrowserBinaries => "Chromium/Chrome binaries downloaded by Puppeteer, Playwright, or Selenium for automated testing. Often 200-700MB each. Safe to delete — re-downloads on next test run.",
            Self::CloudSyncedLocal => "These files are already backed up in the cloud. The local copy is redundant — your cloud provider has it. Removing frees disk while keeping cloud access.",
            Self::StaleProject => "Dev projects with no file changes in 6+ months. Code is likely in git — consider archiving or deleting the working copy.",
            Self::OldDownloads => "Downloads older than 90 days. Often forgotten installers, zips, and PDFs.",
            Self::Trash => "Already-deleted files. Empty Trash to reclaim immediately.",
            Self::DockerData => "Docker images, containers, volumes. Use 'docker system prune -a' for cleanup.",
            Self::IosDeviceBackup => "Full iPhone/iPad backups (10-60GB each). Check dates — delete old ones.",
            Self::VmImages => "Virtual machine disk images. 20-100GB each. Delete VMs you no longer use.",
            Self::LargeMedia => "Video, audio, or image files >200MB. Check if backed up elsewhere.",
            Self::LargeOther => "Large files that don't fit other categories. Review individually.",
            Self::OldAppLeftovers => "Data for apps no longer in /Applications. Likely safe to remove.",
            Self::SimulatorRuntimes => "iOS Simulator runtimes and device support. Delete old OS versions you don't test against.",
            Self::TimeMachineLocal => "Local Time Machine snapshots. Reclaim with 'tmutil deletelocalsnapshots'.",
            Self::CoreDumps => "Process core dumps from crashes. Safe to delete.",
            Self::MailAttachments => "Attachments from Mail. Safe if emails still exist — re-download as needed.",
            Self::HomebrewOldVersions => "Old versions of Homebrew packages and bundled Ruby. Run 'brew cleanup --prune=all' to remove.",
        }
    }

    pub fn quick_command(&self) -> Option<&str> {
        match self {
            Self::Trash => Some("sudo rm -rf /Users/*/.Trash/*"),
            Self::XcodeDerivedData => Some("rm -rf ~/Library/Developer/Xcode/DerivedData/*"),
            Self::PackageManagerCache => Some("npm cache clean --force; pip cache purge; brew cleanup --prune=all"),
            Self::DockerData => Some("docker system prune -a --volumes"),
            Self::CrashReports => Some("rm -rf ~/Library/Logs/DiagnosticReports/*"),
            Self::CoreDumps => Some("sudo rm -rf /cores/*"),
            Self::SimulatorRuntimes => Some("xcrun simctl delete unavailable"),
            Self::BrowserCache => Some("rm -rf ~/Library/Caches/Google/Chrome/Default/Cache/*"),
            Self::CondaInstall => Some("conda clean --all -y"),
            Self::HomebrewOldVersions => Some("brew cleanup --prune=all"),
            Self::RustupToolchains => Some("rustup toolchain list; rustup target list --installed"),
            Self::OldNodeVersions => Some("nvm ls  # then: nvm uninstall <version>"),
            _ => None,
        }
    }

    pub fn risk_level(&self) -> RiskLevel {
        match self {
            Self::PackageManagerCache | Self::AppCache | Self::BrowserCache
            | Self::BuildArtifact | Self::NodeModules | Self::XcodeDerivedData
            | Self::LogsAndDiagnostics | Self::CrashReports | Self::TmpFiles
            | Self::ElectronCache | Self::CoreDumps | Self::SimulatorRuntimes
            | Self::CachedBrowserBinaries | Self::HomebrewOldVersions => RiskLevel::Safe,

            Self::CloudSyncedLocal | Self::StaleProject | Self::OldDownloads
            | Self::Trash | Self::DockerData | Self::MailAttachments
            | Self::TimeMachineLocal | Self::OldAppLeftovers
            | Self::OldNodeVersions | Self::PythonVenvs | Self::OldIdeExtensions
            | Self::DartPubCache | Self::CondaInstall => RiskLevel::ReviewFirst,

            Self::IosDeviceBackup | Self::VmImages | Self::LargeMedia
            | Self::LargeOther | Self::RustupToolchains | Self::AndroidSdk => RiskLevel::Caution,
        }
    }
}
