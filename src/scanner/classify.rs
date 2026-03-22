use crate::model::Category;
use std::fs;
use std::path::Path;

pub fn classify_path(path: &Path, name: &str, is_dir: bool, depth: usize) -> Option<Category> {
    let path_str = path.to_string_lossy();

    // Cloud storage
    if path_str.contains("/Library/CloudStorage/") || path_str.contains("/Google Drive/")
        || path_str.contains("/Dropbox/") || path_str.contains("/OneDrive/")
        || path_str.contains("Library/Mobile Documents/com~apple~CloudDocs")
    {
        return Some(Category::CloudSyncedLocal);
    }

    if name == ".Trash" && is_dir { return Some(Category::Trash); }

    // Docker
    if path_str.contains("com.docker.docker") || (name == ".docker" && is_dir) {
        return Some(Category::DockerData);
    }

    // iOS backups
    if path_str.contains("MobileSync/Backup") { return Some(Category::IosDeviceBackup); }

    // Xcode
    if name == "DerivedData" && path_str.contains("Xcode") { return Some(Category::XcodeDerivedData); }
    if path_str.contains("Developer/CoreSimulator") || path_str.contains("iOS DeviceSupport") {
        return Some(Category::SimulatorRuntimes);
    }

    // ── Runtime version sprawl ──────────────────────────────────────────

    // nvm node versions — each is 150-900MB
    if is_dir && path_str.contains(".nvm/versions/node/") && name.starts_with('v') && depth <= 4 {
        return Some(Category::OldNodeVersions);
    }

    // Python virtual environments (venv, .venv, env)
    if is_dir && (name == "venv" || name == ".venv" || name == "env") {
        if path.join("pyvenv.cfg").exists() {
            return Some(Category::PythonVenvs);
        }
    }

    // Conda/Anaconda installation
    if is_dir && (name == "anaconda3" || name == "anaconda" || name == "miniconda3" || name == "miniconda") {
        if path.join("conda-meta").is_dir() {
            return Some(Category::CondaInstall);
        }
    }
    // Conda pkgs cache specifically
    if name == "pkgs" && is_dir && path.parent().map_or(false, |p| {
        let pname = p.file_name().unwrap_or_default().to_string_lossy();
        pname.contains("conda") || pname.contains("anaconda")
    }) {
        return Some(Category::PackageManagerCache);
    }

    // Rustup toolchains
    if is_dir && path_str.contains(".rustup/toolchains/") && depth <= 4 {
        return Some(Category::RustupToolchains);
    }

    // Old IDE extensions — VS Code, Cursor, Windsurf, JetBrains
    if is_dir && depth <= 3 {
        if (path_str.contains(".vscode/extensions/") || path_str.contains(".cursor/extensions/")
            || path_str.contains(".windsurf/extensions/"))
            && name.contains('-')  // extensions have name-version format
        {
            return Some(Category::OldIdeExtensions);
        }
    }

    // Android SDK components
    if is_dir && path_str.contains("Library/Android/sdk/") && depth <= 4 {
        if name == "system-images" || name == "emulator" || name == "build-tools"
            || name == "platforms" || name == "sources" || name == "ndk"
        {
            return Some(Category::AndroidSdk);
        }
    }

    // Dart/Flutter pub cache
    if name == ".pub-cache" && is_dir { return Some(Category::DartPubCache); }

    // Cached browser binaries (Puppeteer, Playwright, Selenium downloads)
    if is_dir && path_str.contains(".cache/") {
        if name == "puppeteer" || name == "ms-playwright" || name == "selenium" {
            return Some(Category::CachedBrowserBinaries);
        }
    }
    // Codeium / Windsurf browser downloads
    if is_dir && path_str.contains(".codeium/") && name == "ws-browser" {
        return Some(Category::CachedBrowserBinaries);
    }

    // Homebrew old versions (Cellar with multiple versions)
    if is_dir && path_str.contains("/Cellar/") && path_str.starts_with("/opt/homebrew") {
        return Some(Category::HomebrewOldVersions);
    }

    // ── Existing categories ─────────────────────────────────────────────

    // node_modules
    if name == "node_modules" && is_dir { return Some(Category::NodeModules); }

    // Build artifacts
    if name == "target" && is_dir && path.parent().map_or(false, |p| p.join("Cargo.toml").exists()) {
        return Some(Category::BuildArtifact);
    }
    if name == ".build" && is_dir && path.parent().map_or(false, |p| p.join("Package.swift").exists()) {
        return Some(Category::BuildArtifact);
    }
    if name == "build" && is_dir && path.parent().map_or(false, |p| {
        p.join("build.gradle").exists() || p.join("build.gradle.kts").exists()
            || p.join("CMakeLists.txt").exists()
    }) {
        return Some(Category::BuildArtifact);
    }

    // Package manager caches
    if name == "_cacache" && path_str.contains(".npm") { return Some(Category::PackageManagerCache); }
    if path_str.contains("Library/Caches/Homebrew") { return Some(Category::PackageManagerCache); }
    if path_str.contains("Library/Caches/pip") || path_str.contains(".cache/pip") { return Some(Category::PackageManagerCache); }
    if path_str.contains(".cargo/registry") || path_str.contains(".cargo/git") { return Some(Category::PackageManagerCache); }
    if path_str.contains("Library/Caches/CocoaPods") { return Some(Category::PackageManagerCache); }
    if path_str.contains(".gradle/caches") { return Some(Category::PackageManagerCache); }
    if path_str.contains(".m2/repository") { return Some(Category::PackageManagerCache); }
    if path_str.contains(".composer/cache") { return Some(Category::PackageManagerCache); }
    if path_str.contains("go/pkg/mod/cache") { return Some(Category::PackageManagerCache); }
    if name == ".yarn" && is_dir && path.join("cache").exists() { return Some(Category::PackageManagerCache); }
    if name == ".pnpm-store" && is_dir { return Some(Category::PackageManagerCache); }
    // uv cache (Python)
    if is_dir && path_str.contains(".cache/uv") { return Some(Category::PackageManagerCache); }

    // Browser caches
    if is_dir && path_str.contains("Library/Caches/") {
        if name.contains("Chrome") || name.contains("Safari") || name.contains("Firefox")
            || name.contains("brave") || name.contains("edgemac") || name.contains("Arc")
        {
            return Some(Category::BrowserCache);
        }
    }

    // Electron caches
    if is_dir && path_str.contains("Library/Application Support/") {
        if (name == "Cache" || name == "CachedData" || name == "CachedExtensions"
            || name == "GPUCache" || name == "Service Worker")
            && (path_str.contains("/Code/") || path_str.contains("/Slack/")
                || path_str.contains("/discord/") || path_str.contains("/Cursor/"))
        {
            return Some(Category::ElectronCache);
        }
    }

    // Crash reports
    if name == "DiagnosticReports" || name == "CrashReporter" { return Some(Category::CrashReports); }
    if path_str.starts_with("/cores/") { return Some(Category::CoreDumps); }

    // Mail
    if path_str.contains("Mail Downloads") || path_str.contains("Mail/V") { return Some(Category::MailAttachments); }

    // Downloads (age-filtered elsewhere)
    if name == "Downloads" && is_dir && depth == 1 { return Some(Category::OldDownloads); }

    // Logs
    if name == "Logs" && is_dir && (path_str.contains("Library/") || path_str.starts_with("/private/var/")) {
        return Some(Category::LogsAndDiagnostics);
    }

    // Temp
    if path_str.starts_with("/private/tmp") || path_str.starts_with("/tmp") { return Some(Category::TmpFiles); }

    None
}

pub fn is_dev_project(path: &Path) -> bool {
    let markers = [
        "package.json", "Cargo.toml", "go.mod", "pom.xml", "build.gradle",
        "build.gradle.kts", "Makefile", "CMakeLists.txt", "setup.py", "pyproject.toml",
        "Gemfile", "composer.json", "Package.swift", "Podfile", "requirements.txt", "Pipfile",
    ];
    markers.iter().any(|m| path.join(m).exists()) || path.join(".git").is_dir()
}

pub fn has_git_uncommitted(path: &Path) -> Option<bool> {
    let output = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(path)
        .output()
        .ok()?;
    Some(!String::from_utf8_lossy(&output.stdout).trim().is_empty())
}

pub fn get_installed_app_bundle_ids() -> Vec<String> {
    let mut ids = Vec::new();
    for dir in &["/Applications", "/System/Applications"] {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let plist = path.join("Contents/Info.plist");
                if plist.exists() {
                    if let Ok(output) = std::process::Command::new("defaults")
                        .args(["read", &plist.to_string_lossy(), "CFBundleIdentifier"])
                        .output()
                    {
                        let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
                        if !id.is_empty() {
                            ids.push(id);
                        }
                    }
                }
            }
        }
    }
    ids
}
