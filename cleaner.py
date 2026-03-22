import os
import shutil
import click
import subprocess
import stat
from pathlib import Path
from rich.console import Console
from rich.table import Table
from rich.progress import track, Progress, SpinnerColumn, TextColumn, BarColumn, TaskProgressColumn
from rich.prompt import Confirm, Prompt
from rich.panel import Panel

console = Console()

def is_icloud_placeholder(path):
    """
    Checks if a file is an iCloud placeholder using xattr.
    Returns True if 'com.apple.icloud.itemName' exists (Placeholder).
    Returns False if attribute is missing (Local File).
    """
    try:
        subprocess.check_call(
            ["xattr", "-p", "com.apple.icloud.itemName", str(path)], 
            stdout=subprocess.DEVNULL, 
            stderr=subprocess.DEVNULL
        )
        return True
    except (subprocess.CalledProcessError, FileNotFoundError):
        return False

class Cleaner:
    def __init__(self, name, description, user="Current", danger_level="Low"):
        self.name = name
        self.description = description
        self.danger_level = danger_level
        self.user = user
        self.size = 0
        self.file_count = 0
        self.exists = False
        self.is_command = False

    def scan(self):
        raise NotImplementedError

    def clean(self):
        raise NotImplementedError

    def get_size_str(self):
        if not self.exists:
            return "-"
        return format_bytes(self.size)

class PathCleaner(Cleaner):
    def __init__(self, name, path, description, user="Current", danger_level="Low"):
        super().__init__(name, description, user, danger_level)
        self.path = Path(path).expanduser()

    def scan(self):
        if not self.path.exists():
            self.exists = False
            return

        self.exists = True
        self.size = 0
        self.file_count = 0
        
        try:
            if self.path.is_dir():
                for root, dirs, files in os.walk(self.path):
                    for f in files:
                        fp = Path(root) / f
                        try:
                            if fp.is_symlink():
                                continue
                            
                            st = fp.stat()
                            # Use st_blocks * 512 for actual physical size
                            self.size += st.st_blocks * 512
                            self.file_count += 1
                        except (OSError, PermissionError):
                            continue
            elif self.path.is_file():
                 st = self.path.stat()
                 self.size = st.st_blocks * 512
                 self.file_count = 1
        except PermissionError:
            pass

    def clean(self):
        if not self.exists:
            return 0
        
        if self.path.is_dir():
             for item in self.path.iterdir():
                try:
                    if item.is_dir():
                        shutil.rmtree(item)
                    else:
                        os.unlink(item)
                except Exception as e:
                    console.print(f"[red]Error deleting {item}: {e}[/red]")
        elif self.path.is_file():
            try:
                os.unlink(self.path)
            except Exception as e:
                console.print(f"[red]Error deleting {self.path}: {e}[/red]")
        return True

class CloudCleaner(Cleaner):
    def __init__(self, name, path, description, user="Current", danger_level="Medium"):
        super().__init__(name, description, user, danger_level)
        self.path = Path(path).expanduser()

    def scan(self):
        if not self.path.exists():
            self.exists = False
            return
        
        self.exists = True
        self.size = 0
        self.file_count = 0
        
        for root, dirs, files in os.walk(self.path):
            for f in files:
                if f.startswith('.'): continue
                fp = Path(root) / f
                try:
                    # If it IS a placeholder, it takes no space (skip)
                    if is_icloud_placeholder(fp):
                        continue
                    
                    # It is local, so we can evict it
                    self.size += fp.stat().st_blocks * 512
                    self.file_count += 1
                except Exception:
                    continue

    def clean(self):
        if not self.exists: return False
        
        for root, dirs, files in os.walk(self.path):
            for f in files:
                if f.startswith('.'): continue
                fp = Path(root) / f
                if not is_icloud_placeholder(fp):
                    try:
                        subprocess.run(["brctl", "evict", str(fp)], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                    except Exception as e:
                        console.print(f"[red]Failed to evict {fp.name}: {e}[/red]")
        return True

class CommandCleaner(Cleaner):
    def __init__(self, name, check_command, clean_command, description, danger_level="Low"):
        super().__init__(name, description, "System", danger_level)
        self.check_command = check_command
        self.clean_command = clean_command
        self.is_command = True

    def scan(self):
        self.exists = shutil.which(self.check_command.split()[0]) is not None
        self.size = 0
        self.file_count = 0

    def clean(self):
        if not self.exists:
            return False
        try:
            subprocess.run(self.clean_command, shell=True, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            return True
        except subprocess.CalledProcessError:
            console.print(f"[red]Error executing: {self.clean_command}[/red]")
            return False
    
    def get_size_str(self):
        return "N/A"

def get_cleaners():
    cleaners = []
    
    is_root = os.geteuid() == 0
    users_to_scan = []

    if is_root:
        base_users = Path('/Users')
        if base_users.exists():
            for u in os.listdir(base_users):
                if u.startswith('.') or u in ['Shared', 'Guest']:
                    continue
                user_home = base_users / u
                if user_home.is_dir():
                    users_to_scan.append((u, user_home))
    else:
        current_user = os.environ.get('USER', 'Current')
        users_to_scan.append((current_user, Path.home()))

    for username, user_home in users_to_scan:
        cleaners.extend([
            PathCleaner(
                "User Caches", 
                user_home / "Library/Caches", 
                "Temporary files created by apps.",
                user=username,
                danger_level="Low"
            ),
            PathCleaner(
                "User Logs", 
                user_home / "Library/Logs", 
                "Log files from applications.",
                user=username,
                danger_level="Low"
            ),
            PathCleaner(
                "Trash", 
                user_home / ".Trash", 
                "Files already deleted.",
                user=username,
                danger_level="Low"
            ),
            PathCleaner(
                "Xcode DerivedData", 
                user_home / "Library/Developer/Xcode/DerivedData", 
                "Build artifacts.",
                user=username,
                danger_level="Low"
            ),
            PathCleaner(
                "Xcode Simulator Caches",
                user_home / "Library/Developer/CoreSimulator/Caches",
                "Simulator caches.",
                user=username,
                danger_level="Low"
            ),
            PathCleaner(
                "iOS DeviceSupport",
                user_home / "Library/Developer/Xcode/iOS DeviceSupport",
                "iOS Debugging symbols (Huge).",
                user=username,
                danger_level="Medium"
            ),
             PathCleaner(
                "watchOS DeviceSupport",
                user_home / "Library/Developer/Xcode/watchOS DeviceSupport",
                "watchOS Debugging symbols.",
                user=username,
                danger_level="Medium"
            ),
            PathCleaner(
                "tvOS DeviceSupport",
                user_home / "Library/Developer/Xcode/tvOS DeviceSupport",
                "tvOS Debugging symbols.",
                user=username,
                danger_level="Medium"
            ),
            PathCleaner(
                "Application Support Caches",
                user_home / "Library/Application Support/Caches",
                "App Support Caches.",
                user=username,
                danger_level="Medium"
            ),
            
            # Browsers
            PathCleaner(
                "Google Chrome Cache",
                user_home / "Library/Caches/Google/Chrome",
                "Browser cache for Chrome.",
                user=username,
                danger_level="Medium"
            ),
            PathCleaner(
                "Mozilla Firefox Cache",
                user_home / "Library/Caches/Firefox",
                "Browser cache for Firefox.",
                user=username,
                danger_level="Medium"
            ),
            PathCleaner(
                "Safari Cache",
                user_home / "Library/Containers/com.apple.Safari/Data/Library/Caches",
                "Browser cache for Safari.",
                user=username,
                danger_level="Medium"
            ),

            # Communication Apps
            PathCleaner(
                "Discord Cache",
                user_home / "Library/Application Support/discord/Cache",
                "Cache files for Discord.",
                user=username,
                danger_level="Low"
            ),
            PathCleaner(
                "Slack Cache",
                user_home / "Library/Containers/com.tinyspeck.slackmacgap/Data/Library/Application Support/Slack/Cache",
                "Cache files for Slack (App Store).",
                user=username,
                danger_level="Low"
            ),
            PathCleaner(
                "Slack Cache (Direct)",
                user_home / "Library/Application Support/Slack/Cache",
                "Cache files for Slack (Direct).",
                user=username,
                danger_level="Low"
            ),

            # Messages
            PathCleaner(
                "iMessage Attachments",
                user_home / "Library/Messages/Attachments",
                "Images/Videos from chats. Deleting breaks chat history previews.",
                user=username,
                danger_level="High"
            ),

            # System / Misc
            PathCleaner(
                "QuickLook Thumbnails",
                user_home / "Library/Caches/com.apple.QuickLook.thumbnailcache",
                "Cached file thumbnails.",
                user=username,
                danger_level="Low"
            ),
            PathCleaner(
                "Mail Downloads",
                user_home / "Library/Containers/com.apple.mail/Data/Library/Mail Downloads",
                "Local email attachments.",
                user=username,
                danger_level="Medium"
            ),
            
            # iCloud (Optimize Storage)
            CloudCleaner(
                "iCloud Drive (Local)",
                user_home / "Library/Mobile Documents/com~apple~CloudDocs",
                "Evicts local copies of iCloud files (Keep in Cloud).",
                user=username,
                danger_level="Medium"
            )
        ])

    # Command cleaners are system-wide or context-dependent
    cleaners.extend([
        CommandCleaner(
            "Unused Simulators",
            "xcrun",
            "xcrun simctl delete unavailable",
            "Deletes simulators for unsupported runtimes.",
            danger_level="Low"
        ),
        CommandCleaner(
            "Reset Simulators",
            "xcrun",
            "xcrun simctl erase all",
            "Resets all simulators to factory state.",
            danger_level="Medium"
        ),
        CommandCleaner(
            "CocoaPods Cache",
            "pod",
            "pod cache clean --all",
            "Clears CocoaPods cache.",
            danger_level="Low"
        )
    ])
    
    return cleaners

def format_bytes(size):
    power = 2**10
    n = 0
    power_labels = {0 : '', 1: 'K', 2: 'M', 3: 'G', 4: 'T'}
    while size > power:
        size /= power
        n += 1
    return f"{size:.2f} {power_labels[n]}B"

@click.command()
@click.option('--dry-run', is_flag=True, help='Scan and show what would be deleted without deleting.')
def main(dry_run):
    is_root = os.geteuid() == 0
    title = "[bold blue]MacOS System Cleaner[/bold blue]"
    if is_root:
        title += " [bold red](ROOT MODE)[/bold red]"
    
    console.print(Panel.fit(f"{title}\n[italic]Safely reclaim disk space from Caches, Logs, and Dev Tools[/italic]"))

    if is_root:
        console.print("[yellow]Running as root: Scanning ALL user directories in /Users[/yellow]")
    else:
        console.print("[dim]Running as current user. To clean other users, run with 'sudo'.[/dim]")

    cleaners = get_cleaners()
    active_cleaners = []

    with Progress(
        SpinnerColumn(),
        TextColumn("[progress.description]{task.description}"),
        transient=True
    ) as progress:
        task = progress.add_task("Scanning system...", total=len(cleaners))
        
        for cleaner in cleaners:
            progress.update(task, description=f"Scanning {cleaner.name} ({cleaner.user})...")
            cleaner.scan()
            if cleaner.exists:
                if (not cleaner.is_command and cleaner.size > 0) or cleaner.is_command:
                     active_cleaners.append(cleaner)
            progress.advance(task)

    if not active_cleaners:
        console.print("[green]System is clean! Nothing to delete.[/green]")
        return

    table = Table(title="Scan Results")
    table.add_column("Category", style="cyan")
    table.add_column("Type", style="magenta")
    table.add_column("Risk", style="bold")
    table.add_column("Size", justify="right", style="green")
    table.add_column("Description")

    total_size = 0
    has_high_risk = False
    
    for c in active_cleaners:
        size_display = c.get_size_str()
        c_type = "Command" if c.is_command else "Path"
        
        risk_style = "green"
        if c.danger_level == "Medium": risk_style = "yellow"
        if c.danger_level == "High": 
            risk_style = "red"
            has_high_risk = True

        table.add_row(c.user, c.name, c_type, f"[{risk_style}]{c.danger_level}[/{risk_style}]", size_display, c.description)
        if not c.is_command:
            total_size += c.size

    console.print(table)
    console.print(f"\n[bold]Total Reclaimable Space:[/bold] [green]{format_bytes(total_size)}[/green] (plus command-based cleanups)\n")

    if dry_run:
        console.print("[yellow]Dry run complete. No files were deleted.[/yellow]")
        return

    if has_high_risk:
        console.print("[bold red]WARNING: High Risk items detected![/bold red]")
        console.print("Deleting 'High' risk items (like iMessage Attachments) will remove them from your chat history forever.")
        if Prompt.ask("Type 'CONFIRM' to proceed with deletion") != "CONFIRM":
            console.print("Operation cancelled.")
            return
    elif not Confirm.ask("Do you want to proceed with cleaning?"):
        console.print("Operation cancelled.")
        return
    
    with Progress(
        SpinnerColumn(),
        TextColumn("[progress.description]{task.description}"),
        BarColumn(),
        TaskProgressColumn(),
    ) as progress:
        task = progress.add_task("Cleaning...", total=len(active_cleaners))
        
        for cleaner in active_cleaners:
            progress.update(task, description=f"Cleaning {cleaner.name}...")
            cleaner.clean()
            progress.advance(task)

    console.print(f"[bold green]Cleanup Complete![/bold green]")

if __name__ == '__main__':
    main()
