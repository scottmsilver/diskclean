# MacOS Cleaner

A safe, Python-based CLI tool to clean up unnecessary files on macOS. Supports both single-user and system-wide (root) cleaning.

## Features
- **Multi-User Support**: When run with `sudo`, scans and cleans caches for **all** users on the system.
- **Smart Detection**: Detects and ignores iCloud "Optimize Storage" placeholder files to prevent accidental downloads or data loss.
- **Physical Size Calculation**: accurately calculates space using disk blocks.
- **Scans** and **Cleans**:
  - **System/User**:
    - `~/Library/Caches` (General)
    - `~/Library/Logs`
    - `~/.Trash`
    - `~/Library/Caches/com.apple.QuickLook.thumbnailcache`
    - `~/Library/Containers/com.apple.mail/Data/Library/Mail Downloads`
  - **Browsers**:
    - Google Chrome Cache
    - Firefox Cache
    - Safari Cache
  - **Apps**:
    - Discord Cache
    - Slack Cache (App Store & Direct)
    - **iMessage Attachments** (High Risk: Requires explicit confirmation).
    - Application Support Caches
  - **Development (Xcode)**:
    - DerivedData
    - CoreSimulator Caches
    - iOS/watchOS/tvOS DeviceSupport
  - **iCloud Drive**:
    - Evicts local copies of iCloud files (keeps them in the cloud but frees up local disk space).
- **Advanced Command-Based Cleaning**:
  - **Unused Simulators**: `xcrun simctl delete unavailable`
  - **Reset Simulators**: `xcrun simctl erase all`
  - **CocoaPods Cache**: `pod cache clean --all`
- **Dry Run** mode.
- **Interactive** confirmation (with High Risk safeguards).
- **Beautiful UI** using `rich`.

## Installation

1. Ensure you have Python 3 installed.
2. Create a virtual environment (optional but recommended):
   ```bash
   python3 -m venv venv
   source venv/bin/activate
   ```
3. Install dependencies:
   ```bash
   pip install -r requirements.txt
   ```

## Usage

**Dry Run (Recommended first):**
```bash
python3 cleaner.py --dry-run
```

**Clean (Current User):**
```bash
python3 cleaner.py
```

**Clean All Users (System Wide):**
```bash
sudo python3 cleaner.py
```

## Safety
- **iCloud Safe**: Uses `xattr` to detect iCloud placeholders.
- **Strict Scope**: Only targets specific, safe-to-delete directories.
- **Eviction vs Deletion**: For iCloud files, it uses `brctl evict`.
- **High Risk Warnings**: Requires explicit "CONFIRM" typing for risky deletions (like iMessage history).