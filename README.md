# diskclean

Fast macOS disk cleaner with an interactive TUI. Scans 1.3M files in ~5 seconds using `getattrlistbulk`, categorizes everything semantically, and cleans up with honest before/after verification.

## Features

- **Fast**: 15x faster than `du` — uses macOS `getattrlistbulk()` with rayon work-stealing across all cores
- **30+ categories**: conda, venvs, node_modules, IDE extensions, brew, Docker, simulators, cloud-synced files, duplicates, and more
- **Duplicate detection**: groups by size, hashes first+last 4KB, skips APFS clones/hardlinks
- **Dedup via hardlinks**: identical cache files get hardlinked — frees space without deleting anything
- **Honest estimates**: measures actual `statfs` delta, reports failures truthfully
- **Interactive TUI**: browse categories, expand to see items, queue cleanups, execute in background
- **Optional AI safety**: set `GEMINI_API_KEY` to get Gemini 3.1 Pro safety assessments before cleaning
- **Physical sizes**: uses `st_blocks * 512` (actual disk usage), skips iCloud-evicted dataless files

## Install

```bash
git clone https://github.com/scottmsilver/diskclean.git
cd diskclean
cargo build --release
```

The binary will be at `./target/aarch64-apple-darwin/release/diskclean`.

> On Apple Silicon Macs, the build defaults to `aarch64-apple-darwin`. On Intel Macs, remove `.cargo/config.toml` or set the target manually.

## Usage

```bash
# Interactive TUI (prompts for sudo for full scanning)
./target/aarch64-apple-darwin/release/diskclean

# Skip sudo (scan only what your user can access)
./target/aarch64-apple-darwin/release/diskclean --no-sudo

# Plain text output (for piping/scripting)
./target/aarch64-apple-darwin/release/diskclean --plain --no-sudo
```

### TUI Keybindings

| Key | Action |
|-----|--------|
| `↑/↓` or `j/k` | Navigate categories |
| `Enter` | Expand/collapse category |
| `c` | Open cleanup strategy picker |
| `Enter` (in picker) | Add job to queue |
| `a` (in picker) | Ask Gemini for safety assessment |
| `X` | Execute all queued jobs |
| `J` | Toggle jobs panel |
| `?` | Help |
| `q` | Quit |

### Cleanup Flow

1. Browse categories, expand with `Enter` to inspect items
2. Press `c` — pick a strategy (Official tool command, Dedup, or Delete)
3. `Enter` adds to the job queue (nothing runs yet)
4. Repeat for other categories
5. `X` executes all queued jobs in parallel
6. Jobs panel shows real-time: `pending → running → done` with actual bytes freed

## What It Finds

| Category | Examples |
|----------|---------|
| Package Caches | npm, pip, brew, cargo, uv, CocoaPods, gradle |
| Conda/Anaconda | Full install + cached packages |
| Python Venvs | Scattered venv/.venv in projects |
| Old Node Versions | nvm-managed versions |
| Old IDE Extensions | VS Code, Cursor, Windsurf |
| Rust Toolchains | Unused rustup targets |
| Build Artifacts | target/, .build/, build/ |
| node_modules | Per-project JS dependencies |
| Xcode DerivedData | Build caches |
| Simulator Runtimes | Old iOS simulator versions |
| Android SDK | Emulators, build tools |
| Docker | Images, containers, volumes |
| App Caches | ~/Library/Caches per-app |
| Browser Caches | Chrome, Safari, Firefox, Arc |
| Cloud-Synced | Google Drive, iCloud, Dropbox local copies |
| Stale Projects | Git repos untouched >6 months |
| Old Downloads | ~/Downloads older than 90 days |
| Duplicates | Identical content in multiple locations |
| Cached Browsers | Puppeteer/Selenium Chromium downloads |
| System Temp | /var/folders old caches |
| Logs & Crashes | System/app logs, crash reports, core dumps |
| App Leftovers | Data for uninstalled apps |

## How It Works

### Scanning

Uses macOS `getattrlistbulk()` — one syscall returns all metadata for every entry in a directory. Combined with:

- **rayon work-stealing** at every directory level
- **openat()** to skip kernel path resolution
- **Thread-local 1MB buffers** to avoid allocation per directory
- **DashMap** for lock-free bucket accumulation
- **Single-pass**: classify + accumulate sizes in one walk, no double-traversal

### Duplicate Detection

1. Collect `(physical_size → path)` for unbucketed files >1MB
2. Group by size — most files are unique, very few need hashing
3. Hash: first 4KB + last 4KB (8KB per file)
4. Skip hardlinks and APFS clones
5. Reclaimable = total - largest copy

### Verification

Every cleanup measures `statfs` before and after. If disk free space doesn't change, it reports failure honestly.

## Performance

MacBook Pro M1, 245GB APFS, FileVault:

| Metric | Value |
|--------|-------|
| Files scanned | 1.3M |
| Scan time (arm64) | ~5s |
| Memory | ~20MB |

## License

MIT
