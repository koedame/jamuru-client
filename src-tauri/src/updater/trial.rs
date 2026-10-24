//! The version that ran before an update is kept until the new one has
//! started (ADR-088).
//!
//! An update replaces the app where it is installed. If the new version cannot
//! start - it does not run on this machine, crashes while starting, or hangs
//! before its screen appears - the app would be gone, and starting it again
//! would only fail again. So before the install the installed package is
//! copied aside (the slot), and a watcher sees the new version through its
//! first start. Whoever decides first - the new version saying it has started,
//! or the watcher giving up on it - writes the decision, and the other follows
//! it. A version given up on is copied back from the slot, the old version is
//! started again, and the given-up version is not installed again by itself.
//!
//! The watcher is the old version's own program, run from the slot, so it runs
//! where the new one may not. It is a process of its own with nothing of the
//! app in it - no window, no audio, no network - started before the install
//! and alive until the trial is decided.
//!
//! Who is alive is told by file locks, not process ids: a lock is let go of by
//! the system when its process ends, however it ends, and a process id can be
//! given to another program.
//!
//! Everything here is files in one directory ([`Store`]), so the old version,
//! the new one and the watcher, which never run the same code, agree only on
//! that layout.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// The layout of the files below. The watcher of a trial is the version that
/// opened it, so a later version must still take part in a trial of every
/// earlier protocol: changing what a file means is changing what the installed
/// old version does with the next release.
const PROTOCOL: u32 = 1;

/// Starts the program as the watcher: `<program> --update-watch <dir> <trial id>`.
const WATCH_ARG: &str = "--update-watch";

const TRIAL: &str = "trial.json";
const SLOT: &str = "slot";
const SLOT_PARTIAL: &str = "slot.partial";
/// The decision of a trial is `decision-<trial id>`: one written for a trial
/// that has since closed is never read as another's.
const DECISION: &str = "decision-";
/// The old version wrote this once the install had returned.
const INSTALLED: &str = "installed";
/// The old version wrote this as it ended to let the new one start.
const HANDOVER: &str = "handover";
/// The new version wrote this, with its process id, once it held its lock.
const STARTED: &str = "started";
/// The new version wrote this when the person quit it before it had started.
const QUIT: &str = "quit";
/// How many times a watcher has begun putting the old version back.
const ATTEMPTS: &str = "attempts";
/// How many times the new version has started in its trial, not counting a
/// start the person quit.
const STARTS: &str = "starts";
/// The watcher wrote this, with the id of its trial, once it held its lock.
const WATCHING: &str = "watching";
/// What a trial writes besides its decision, removed when it closes and
/// before the next one opens.
const MARKS: [&str; 7] = [
    INSTALLED, HANDOVER, STARTED, QUIT, ATTEMPTS, STARTS, WATCHING,
];
/// Held by the old version from opening the trial until it hands over or ends.
const FORMER_LOCK: &str = "former.lock";
/// Held by the new version while it runs during the trial.
const CANDIDATE_LOCK: &str = "candidate.lock";
/// Held by the watcher while it runs.
const WATCHER_LOCK: &str = "watcher.lock";
const REJECTED: &str = "rejected.json";
/// The release the app last installed by itself, and when.
const BY_ITSELF: &str = "by-itself.json";
/// Windows: the watcher runs from a copy of the old program under another
/// name, since the installer ends every process of the app's own name.
#[cfg(windows)]
const WATCHER_DIR: &str = "watcher";
#[cfg(windows)]
const WATCHER_PROGRAM: &str = "jamuru-update.exe";
const LOG: &str = "watch.log";
const LOG_LIMIT: u64 = 256 * 1024;
/// Versions given up on that are remembered; older ones are dropped.
const REJECTED_KEPT: usize = 8;
/// Watchers that may begin putting the old version back before the trial is
/// given up as it is: one that cannot put it back must not keep the app from
/// starting at all.
const MAX_ATTEMPTS: u32 = 3;
/// Starts of the new version before it gives up on itself. The count is kept
/// in the store because a watcher does not outlive the system: a new version
/// that takes the whole machine down with it would otherwise be tried on
/// every start from then on.
const MAX_STARTS: u32 = 3;
/// How long a watcher that was started has to hold its lock.
const WATCHER_UP: Duration = Duration::from_secs(30);
/// How long a starting new version waits for its lock: the copy it restarted
/// from may still be ending.
const CANDIDATE_WAIT: Duration = Duration::from_secs(5);
/// How long the old version waits for its lock: something only looking at it
/// may hold it for a moment.
const FORMER_WAIT: Duration = Duration::from_secs(1);
const LOCK_RETRY: Duration = Duration::from_millis(20);

/// How long the watcher gives each step.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    /// From the old version handing over to the new one starting. On Windows
    /// it covers the install itself, which the installer does after the old
    /// app has ended.
    pub start: Duration,
    /// From the new version starting to its saying it has started.
    pub confirm: Duration,
    /// How long the new version may be gone before it counts as ended: one
    /// that restarts itself takes its lock again within this.
    pub settle: Duration,
    /// How long a version that was ended has to let go of its lock.
    pub kill: Duration,
    pub poll: Duration,
}

impl Limits {
    pub(crate) const RELEASE: Limits = Limits {
        start: if cfg!(windows) {
            Duration::from_secs(180)
        } else {
            Duration::from_secs(60)
        },
        confirm: Duration::from_secs(180),
        settle: Duration::from_secs(10),
        kill: Duration::from_secs(10),
        poll: Duration::from_millis(250),
    };
}

/// What the app is installed as: the entries of `root` that an update
/// replaces, and how to start it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Package {
    pub root: PathBuf,
    /// Names directly under `root`. Only these are copied and put back: on
    /// Windows `root` also holds the app's data.
    pub entries: Vec<String>,
    /// Under `root`: what is opened to start the app (the `.app` on macOS).
    pub launch: PathBuf,
    /// Under `root`: the program itself.
    pub program: PathBuf,
}

impl Package {
    /// The package this process runs from.
    pub(crate) fn running() -> Result<Package, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let appimage = std::env::var_os("APPIMAGE").map(PathBuf::from);
        Self::locate(std::env::consts::OS, &exe, appimage.as_deref(), |p| {
            p.exists()
        })
    }

    /// The package of a program at `exe`, laid out as each platform's
    /// self-updating bundle is: an AppImage on Linux, an `.app` on macOS, the
    /// per-user install directory on Windows.
    fn locate(
        os: &str,
        exe: &Path,
        appimage: Option<&Path>,
        exists: impl Fn(&Path) -> bool,
    ) -> Result<Package, String> {
        match os {
            "linux" => {
                let image = appimage.ok_or("not running from an AppImage")?;
                let (root, name) = split(image)?;
                Ok(Package {
                    root,
                    entries: vec![name.clone()],
                    launch: name.clone().into(),
                    program: name.into(),
                })
            }
            "macos" => {
                let macos = exe.parent().filter(|p| p.ends_with("Contents/MacOS"));
                let bundle = macos
                    .and_then(Path::parent)
                    .and_then(Path::parent)
                    .filter(|p| p.extension() == Some(OsStr::new("app")))
                    .ok_or("not running from an .app bundle")?;
                let (root, name) = split(bundle)?;
                let binary = exe.file_name().ok_or("the program has no name")?;
                Ok(Package {
                    root,
                    entries: vec![name.clone()],
                    launch: name.clone().into(),
                    program: Path::new(&name).join("Contents/MacOS").join(binary),
                })
            }
            "windows" => {
                let (root, name) = split(exe)?;
                let mut entries = vec![name.clone()];
                for other in ["resources", "uninstall.exe"] {
                    if exists(&root.join(other)) {
                        entries.push(other.to_string());
                    }
                }
                Ok(Package {
                    root,
                    entries,
                    launch: name.clone().into(),
                    program: name.into(),
                })
            }
            other => Err(format!("updates are not kept aside on {}", other)),
        }
    }

    /// Whether the app can write where it is installed and the package
    /// itself. A trial is only opened where the old version can be put back
    /// without asking for a password: an `.app` the person cannot write is
    /// replaced by the updater only with an administrator's password, which
    /// the watcher does not have.
    pub(crate) fn writable(&self) -> io::Result<()> {
        let probe = self
            .root
            .join(format!(".jamuru-update-probe-{}", std::process::id()));
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)?;
        fs::remove_file(&probe)?;
        for entry in &self.entries {
            let path = self.root.join(entry);
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStrExt;
                let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
                // SAFETY: a NUL-terminated path that outlives the call, which
                // only reads it.
                if unsafe { libc::access(c_path.as_ptr(), libc::W_OK) } != 0 {
                    let e = io::Error::last_os_error();
                    return Err(io::Error::new(
                        e.kind(),
                        format!("{}: {}", path.display(), e),
                    ));
                }
            }
            // Windows does not hold a directory's read-only mark against
            // writing into it.
            #[cfg(windows)]
            if fs::metadata(&path).is_ok_and(|m| m.is_file() && m.permissions().readonly()) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("{} is read-only", path.display()),
                ));
            }
        }
        Ok(())
    }
}

fn split(path: &Path) -> Result<(PathBuf, String), String> {
    let root = path.parent().ok_or("the package has no parent directory")?;
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or("the package has no usable name")?;
    Ok((root.to_path_buf(), name.to_string()))
}

/// An update that has been opened and not yet decided.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Trial {
    pub protocol: u32,
    /// Tells this trial from any other, for a watcher started for it.
    pub id: String,
    pub from: String,
    pub to: String,
    pub package: Package,
}

/// How a trial ended. The first one written stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Decision {
    /// The new version started.
    Confirmed,
    /// The new version did not start; the old one is put back.
    Rejected,
    /// The install did not happen, or the trial no longer applies.
    Abandoned,
}

impl Decision {
    fn word(self) -> &'static str {
        match self {
            Decision::Confirmed => "confirmed",
            Decision::Rejected => "rejected",
            Decision::Abandoned => "abandoned",
        }
    }

    fn from_word(word: &str) -> Option<Decision> {
        match word.trim() {
            "confirmed" => Some(Decision::Confirmed),
            "rejected" => Some(Decision::Rejected),
            "abandoned" => Some(Decision::Abandoned),
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct ByItself {
    version: String,
    /// Seconds since the Unix epoch.
    at: u64,
}

/// A lock this process holds; let go of when dropped or when the process ends.
#[derive(Debug)]
struct Held {
    _file: fs::File,
}

/// What the trial file holds.
#[derive(Debug)]
enum Found {
    Missing,
    Open(Trial),
    /// A trial of another protocol: a later version's, after going back to
    /// this one by hand. Left for that version.
    Foreign(u32),
    /// Not a trial at all. Nothing written here is ever half written, so this
    /// is a damaged disk or a hand.
    Damaged(String),
    /// It could not be read; it may be fine.
    Unreadable(io::Error),
}

/// The directory where a trial lives.
#[derive(Debug, Clone)]
pub(crate) struct Store {
    dir: PathBuf,
}

impl Store {
    /// The app's own store, next to its other local data.
    pub(crate) fn locate() -> Option<Store> {
        directories::ProjectDirs::from("", "", "jamuru").map(|dirs| Store {
            dir: dirs.data_local_dir().join("update"),
        })
    }

    fn at(dir: impl Into<PathBuf>) -> Store {
        Store { dir: dir.into() }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn has(&self, name: &str) -> bool {
        self.path(name).exists()
    }

    fn find(&self) -> Found {
        let text = match fs::read_to_string(self.path(TRIAL)) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Found::Missing,
            Err(e) => return Found::Unreadable(e),
        };
        // The protocol first: a trial of a later one need not read as this
        // one's.
        #[derive(Deserialize)]
        struct Header {
            protocol: u32,
        }
        match serde_json::from_str::<Header>(&text) {
            Ok(header) if header.protocol != PROTOCOL => Found::Foreign(header.protocol),
            Ok(_) => match serde_json::from_str::<Trial>(&text) {
                Ok(trial) => Found::Open(trial),
                Err(e) => Found::Damaged(e.to_string()),
            },
            Err(e) => Found::Damaged(e.to_string()),
        }
    }

    /// Whether a trial is open, or something that may be one: a new update is
    /// not opened over a trial nobody can read.
    pub(crate) fn is_open(&self) -> bool {
        !matches!(self.find(), Found::Missing)
    }

    /// Whether the trial `id` is the one open.
    fn is_trial(&self, id: &str) -> bool {
        matches!(self.find(), Found::Open(trial) if trial.id == id)
    }

    fn decision(&self, id: &str) -> Option<Decision> {
        fs::read_to_string(self.path(&format!("{}{}", DECISION, id)))
            .ok()
            .and_then(|word| Decision::from_word(&word))
    }

    /// Writes `decision` for the trial `id` unless one was written first.
    /// Returns the decision that stands, or `None` when the trial is no
    /// longer open: one decided and closed meanwhile must not be decided again.
    fn decide(&self, id: &str, decision: Decision) -> io::Result<Option<Decision>> {
        // Written whole beside it and linked into place, which fails when the
        // name exists: nobody reads a half-written decision, and of two
        // written at once exactly one is linked.
        let target = self.path(&format!("{}{}", DECISION, id));
        let staged = staged(&target);
        fs::write(&staged, decision.word())?;
        let linked = fs::hard_link(&staged, &target);
        let _ = fs::remove_file(&staged);
        match linked {
            // A trial is closed before its decision is removed, so a decision
            // linked after that removal finds the trial closed.
            Ok(()) if self.is_trial(id) => Ok(Some(decision)),
            Ok(()) => {
                let _ = fs::remove_file(&target);
                Ok(None)
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => match self.decision(id) {
                Some(standing) => Ok(Some(standing)),
                None if !self.is_trial(id) => Ok(None),
                None => Err(io::Error::other("a decision exists but cannot be read")),
            },
            Err(e) => Err(e),
        }
    }

    /// Removes what a trial wrote. The trial file is not among it.
    fn unmark_all(&self) -> io::Result<()> {
        for name in MARKS {
            self.unmark(name)?;
        }
        let entries = match fs::read_dir(&self.dir) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            entries => entries?,
        };
        for entry in entries {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with(DECISION) {
                remove_tree(&entry.path())?;
            }
        }
        Ok(())
    }

    /// A signal between the processes of a trial, which a power loss may
    /// take back: whoever starts after one finds the trial as it was before.
    fn mark(&self, name: &str, text: &str) -> io::Result<()> {
        write_whole(&self.path(name), text.as_bytes(), false)
    }

    /// What a start after a power loss needs to know of the install.
    fn record(&self, name: &str, text: &str) -> io::Result<()> {
        write_whole(&self.path(name), text.as_bytes(), true)
    }

    fn unmark(&self, name: &str) -> io::Result<()> {
        match fs::remove_file(self.path(name)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// Takes the lock `name`, waiting up to `within` for whoever holds it.
    fn take(&self, name: &str, within: Duration) -> io::Result<Option<Held>> {
        fs::create_dir_all(&self.dir)?;
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.path(name))?;
        let began = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Some(Held { _file: file })),
                Err(fs::TryLockError::WouldBlock) if began.elapsed() < within => {
                    std::thread::sleep(LOCK_RETRY)
                }
                Err(fs::TryLockError::WouldBlock) => return Ok(None),
                Err(fs::TryLockError::Error(e)) => return Err(e),
            }
        }
    }

    /// Whether some process holds the lock `name`. Looked at with a shared
    /// lock, so that two who only look do not see each other as holding it.
    fn held(&self, name: &str) -> bool {
        let file = match fs::File::open(self.path(name)) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return false,
            // A lock that cannot be looked at counts as held: the answer
            // decides whether files are replaced under a running program.
            Err(_) => return true,
        };
        !matches!(file.try_lock_shared(), Ok(()))
    }

    fn watcher_alive(&self) -> bool {
        self.held(WATCHER_LOCK)
    }

    /// The versions given up on, the latest last.
    fn rejected(&self) -> Vec<String> {
        fs::read_to_string(self.path(REJECTED))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub(crate) fn is_rejected(&self, version: &str) -> bool {
        self.rejected().iter().any(|v| v == version)
    }

    fn remember_rejected(&self, version: &str) -> io::Result<()> {
        let mut versions = self.rejected();
        versions.retain(|v| v != version);
        versions.push(version.to_string());
        let drop = versions.len().saturating_sub(REJECTED_KEPT);
        versions.drain(..drop);
        write_whole(
            &self.path(REJECTED),
            serde_json::to_string(&versions)?.as_bytes(),
            true,
        )
    }

    fn forget_rejected(&self, version: &str) -> io::Result<()> {
        let mut versions = self.rejected();
        if !versions.iter().any(|v| v == version) {
            return Ok(());
        }
        versions.retain(|v| v != version);
        write_whole(
            &self.path(REJECTED),
            serde_json::to_string(&versions)?.as_bytes(),
            true,
        )
    }

    /// Notes that the app begins to install `version` by itself now. It
    /// stands whether or not the install then works: one that fails every
    /// time would otherwise be downloaded again on every start.
    pub(crate) fn tried_by_itself(&self, version: &str) -> io::Result<()> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let text = serde_json::to_string(&ByItself {
            version: version.to_string(),
            at: stamp,
        })?;
        write_whole(&self.path(BY_ITSELF), text.as_bytes(), true)
    }

    /// Whether the app began to install `version` by itself less than
    /// `within` ago.
    pub(crate) fn tried_by_itself_lately(&self, version: &str, within: Duration) -> bool {
        let Some(noted) = fs::read_to_string(self.path(BY_ITSELF))
            .ok()
            .and_then(|text| serde_json::from_str::<ByItself>(&text).ok())
        else {
            return false;
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        noted.version == version && now.saturating_sub(noted.at) < within.as_secs()
    }

    /// The count kept in `name`; none is 0.
    fn count(&self, name: &str) -> u32 {
        fs::read_to_string(self.path(name))
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
            .unwrap_or(0)
    }

    /// Counts one more watcher beginning to put the old version back, and
    /// returns the count.
    fn attempt(&self) -> u32 {
        let count = self.count(ATTEMPTS) + 1;
        let _ = self.mark(ATTEMPTS, &count.to_string());
        count
    }

    /// Waits up to `within` for the watcher of the trial `id` to hold its
    /// lock. A trial that closed meanwhile was watched to its end.
    fn watched_by(&self, id: &str, within: Duration) -> io::Result<()> {
        let began = Instant::now();
        loop {
            if fs::read_to_string(self.path(WATCHING)).is_ok_and(|watching| watching == id)
                || !self.is_trial(id)
            {
                return Ok(());
            }
            if began.elapsed() >= within {
                return Err(io::Error::other(format!(
                    "the watcher did not start within {:?}",
                    within
                )));
            }
            std::thread::sleep(LOCK_RETRY);
        }
    }

    /// Ends the open trial: the trial file goes first, so whoever looks next
    /// finds none, then what was kept for it. The locks stay; they are only
    /// ever held, not read.
    fn close(&self) -> io::Result<()> {
        self.unmark(TRIAL)?;
        // A power loss must not bring back a trial whose copy is half gone.
        sync_dir(&self.dir)?;
        self.unmark_all()?;
        remove_tree(&self.path(SLOT))?;
        remove_tree(&self.path(SLOT_PARTIAL))?;
        // The watcher on Windows may still be running from here; what is left
        // is removed when the next trial opens.
        #[cfg(windows)]
        let _ = remove_tree(&self.path(WATCHER_DIR));
        Ok(())
    }

    /// Appends a line to the update's own log, which is all there is to read
    /// of what a watcher did: it has no window, and the app's log is not its
    /// own.
    fn note(&self, line: &str) {
        let path = self.path(LOG);
        if fs::metadata(&path).is_ok_and(|m| m.len() > LOG_LIMIT) {
            let _ = fs::remove_file(&path);
        }
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{} [{}] {}", stamp, std::process::id(), line);
        }
    }
}

/// A name beside `path` that no other writer, in this process or another,
/// uses at the same time.
fn staged(path: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut name = path.as_os_str().to_owned();
    name.push(format!(
        ".{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    PathBuf::from(name)
}

/// Writes `bytes` to `path` so that a reader finds the old content or the new,
/// never a part. `durable` also has it on the disk when this returns.
fn write_whole(path: &Path, bytes: &[u8], durable: bool) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let staged = staged(path);
    let written = fs::File::create(&staged).and_then(|mut file| {
        file.write_all(bytes)?;
        if durable {
            file.sync_all()?;
        }
        Ok(())
    });
    written
        .and_then(|()| fs::rename(&staged, path))
        .inspect_err(|_| {
            let _ = fs::remove_file(&staged);
        })?;
    if durable {
        sync_dir(dir)?;
    }
    Ok(())
}

/// Writes a directory's entries to the disk, so that what was created in it
/// or moved into it is still there after a power loss. Windows keeps them by
/// its file system's journal.
fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    fs::File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

fn remove_tree(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
    }
}

/// Copies a file or a directory tree, keeping permissions and, on Unix,
/// symbolic links as links. The copy is on the disk when this returns: one
/// that is relied on after a power loss must not come back empty.
fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        #[cfg(unix)]
        return std::os::unix::fs::symlink(fs::read_link(from)?, to);
        #[cfg(not(unix))]
        return copy_file(from, to);
    }
    if meta.is_dir() {
        fs::create_dir(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        sync_dir(to)?;
        // After the content: a directory that cannot be written to would
        // otherwise refuse it.
        return fs::set_permissions(to, meta.permissions());
    }
    copy_file(from, to)
}

fn copy_file(from: &Path, to: &Path) -> io::Result<()> {
    // Unix writes a file to the disk through any handle, even one only for
    // reading, so the system's own copy is used, with all it keeps of the
    // file (on macOS its extended attributes too).
    #[cfg(unix)]
    {
        fs::copy(from, to)?;
        fs::File::open(to)?.sync_all()
    }
    // Windows does only through a handle that may write, which a read-only
    // file does not give: its content goes first, its attributes last.
    #[cfg(not(unix))]
    {
        let mut target = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(to)?;
        io::copy(&mut fs::File::open(from)?, &mut target)?;
        target.sync_all()?;
        drop(target);
        fs::set_permissions(to, fs::metadata(from)?.permissions())
    }
}

/// Puts what was kept for `package` in `slot` back in place, entry by entry.
/// Each entry is copied next to where it goes and moved over it, so an entry
/// is either the new version's or the old one's, never a mix, and it is on
/// the disk when this returns.
fn restore(package: &Package, slot: &Path) -> io::Result<()> {
    for entry in &package.entries {
        let target = package.root.join(entry);
        let staged = package.root.join(format!(".{}.jamuru-restore", entry));
        remove_tree(&staged)?;
        copy_tree(&slot.join(entry), &staged)?;
        let kept = fs::symlink_metadata(&staged)?;
        let there = fs::symlink_metadata(&target).ok();
        match there {
            Some(there) if kept.is_dir() && there.is_dir() => replace_dir(&staged, &target)?,
            // A file is moved over a file in one step.
            Some(there) if kept.is_file() && there.is_file() => fs::rename(&staged, &target)?,
            // The update made it another kind of entry, which nothing moves
            // over: it is removed first.
            Some(_) => {
                remove_tree(&target)?;
                fs::rename(&staged, &target)?;
            }
            None => fs::rename(&staged, &target)?,
        }
        sync_dir(&package.root)?;
    }
    Ok(())
}

/// Replaces the directory `target` with `staged`. macOS swaps the two in one
/// step, so an `.app` is never missing; elsewhere, and on a file system that
/// cannot swap, `target` is moved aside first and moved back if `staged`
/// cannot take its place.
fn replace_dir(staged: &Path, target: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    if swap(staged, target).is_ok() {
        // `staged` now holds what was at `target`.
        let _ = remove_tree(staged);
        return Ok(());
    }
    let mut aside = target.as_os_str().to_owned();
    aside.push(".jamuru-rejected");
    let aside = PathBuf::from(aside);
    remove_tree(&aside)?;
    fs::rename(target, &aside)?;
    if let Err(e) = fs::rename(staged, target) {
        let _ = fs::rename(&aside, target);
        return Err(e);
    }
    let _ = remove_tree(&aside);
    Ok(())
}

#[cfg(target_os = "macos")]
fn swap(a: &Path, b: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let a = CString::new(a.as_os_str().as_bytes())?;
    let b = CString::new(b.as_os_str().as_bytes())?;
    // SAFETY: both are NUL-terminated paths that outlive the call, which only
    // reads them.
    if unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_SWAP) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// The program the watcher of `trial` runs.
fn watcher_program(store: &Store, trial: &Trial) -> PathBuf {
    #[cfg(windows)]
    {
        let _ = trial;
        store.path(WATCHER_DIR).join(WATCHER_PROGRAM)
    }
    #[cfg(not(windows))]
    store.path(SLOT).join(&trial.package.program)
}

/// The old version's side of a trial, from just before the install until it
/// hands over to the new version.
#[derive(Debug)]
pub(crate) struct Opened {
    store: Store,
    trial: Trial,
    former: Option<Held>,
}

/// Copies `package` aside and opens a trial of `to` over `from`, then starts
/// its watcher. Nothing of the package is changed here. Without a trial the
/// update is not installed: there would be no way back.
pub(crate) fn open(
    store: &Store,
    package: Package,
    from: &str,
    to: &str,
    start_watcher: impl Fn(&Store, &Trial) -> io::Result<()>,
) -> io::Result<Opened> {
    if from == to {
        return Err(io::Error::other("the update is the running version"));
    }
    if store.is_open() {
        return Err(io::Error::other("an earlier update is still being tried"));
    }
    package.writable()?;
    // Taken before anything is kept: from here until it hands over, the old
    // version is alive for whoever looks.
    let former = store
        .take(FORMER_LOCK, FORMER_WAIT)?
        .ok_or_else(|| io::Error::other("another copy of the app is updating"))?;

    let partial = store.path(SLOT_PARTIAL);
    remove_tree(&partial)?;
    fs::create_dir_all(&partial)?;
    for entry in &package.entries {
        copy_tree(&package.root.join(entry), &partial.join(entry))?;
    }
    sync_dir(&partial)?;
    remove_tree(&store.path(SLOT))?;
    fs::rename(&partial, store.path(SLOT))?;
    sync_dir(&store.dir)?;
    #[cfg(windows)]
    {
        let dir = store.path(WATCHER_DIR);
        let _ = remove_tree(&dir);
        fs::create_dir_all(&dir)?;
        copy_file(
            &package.root.join(&package.program),
            &dir.join(WATCHER_PROGRAM),
        )?;
    }
    store.unmark_all()?;

    let trial = Trial {
        protocol: PROTOCOL,
        id: trial_id(),
        from: from.to_string(),
        to: to.to_string(),
        package,
    };
    let text = serde_json::to_string_pretty(&trial).map_err(io::Error::other)?;
    write_whole(&store.path(TRIAL), text.as_bytes(), true)?;
    let mut opened = Opened {
        store: store.clone(),
        trial,
        former: Some(former),
    };
    if let Err(e) = start_watcher(store, &opened.trial) {
        // Without a watcher there is nobody to put the old version back.
        let _ = opened.abandon(false);
        return Err(e);
    }
    store.note(&format!(
        "opened {}: {} over {}",
        opened.trial.id, opened.trial.to, opened.trial.from
    ));
    Ok(opened)
}

fn trial_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    format!("{:x}-{}", millis, std::process::id())
}

impl Opened {
    /// The install returned without an error.
    pub(crate) fn installed(&self) -> io::Result<()> {
        self.store.record(INSTALLED, &self.trial.to)
    }

    /// The old version is about to end so that the new one starts. From here
    /// the watcher counts the time to the new version's start.
    pub(crate) fn hand_over(&mut self) -> io::Result<()> {
        let marked = self.store.record(HANDOVER, &self.trial.to);
        self.former = None;
        marked
    }

    /// The install did not happen. The trial is closed and its watcher leaves.
    /// `put_back` copies the kept version back first, for an install that may
    /// have left the package half replaced. When that fails the trial stays
    /// open, with the lock: the watcher puts the kept version back once this
    /// process has ended.
    pub(crate) fn abandon(&mut self, put_back: bool) -> io::Result<()> {
        if put_back {
            restore(&self.trial.package, &self.store.path(SLOT))?;
        }
        match self.store.decide(&self.trial.id, Decision::Abandoned)? {
            Some(Decision::Abandoned) => {}
            Some(decided) => {
                return Err(io::Error::other(format!(
                    "the update was {} first",
                    decided.word()
                )))
            }
            // Closed by someone else meanwhile: nothing is left to do.
            None => {
                self.former = None;
                return Ok(());
            }
        }
        self.former = None;
        self.store.note(&format!("abandoned {}", self.trial.id));
        self.store.close()
    }
}

/// What a starting app does about a trial, decided from what it finds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Boot {
    /// No trial concerns this start.
    Normal,
    /// This is the new version of the open trial, on its first start.
    Candidate,
    /// The watcher finishes the trial; this start ends at once so as not to
    /// run while the package is being put back.
    HandToWatcher,
    /// The trial is over or no longer applies; closed here. `reject` also
    /// remembers its new version as one that did not start.
    Close { reject: bool },
}

/// What a start of `me` does about `trial`.
fn boot(
    trial: &Trial,
    decision: Option<Decision>,
    me: &str,
    former_alive: bool,
    handed_over: bool,
    watcher_alive: bool,
) -> Boot {
    let ours = me == trial.from || me == trial.to;
    match decision {
        Some(Decision::Confirmed) | Some(Decision::Abandoned) => Boot::Close { reject: false },
        // Put back or being put back: whoever began it may have ended before
        // it was done, and a new watcher finishes it.
        Some(Decision::Rejected) if ours || watcher_alive => Boot::HandToWatcher,
        // Something else was installed since.
        Some(Decision::Rejected) => Boot::Close { reject: true },
        // The old version is still running (after an install without a
        // restart): this is a second copy, which the one running takes over.
        None if former_alive => Boot::Normal,
        None if me == trial.to => Boot::Candidate,
        // The old version starts after it handed over: the new one did not
        // take its place, or not wholly (an installer that ended halfway,
        // with the system), so what was kept is put back as it was.
        None if me == trial.from && (watcher_alive || handed_over) => Boot::HandToWatcher,
        None => Boot::Close { reject: false },
    }
}

/// The new version's side of a trial, while it runs.
#[derive(Debug)]
pub(crate) struct Candidate {
    store: Store,
    trial: Trial,
    _held: Held,
}

/// What a starting app found and did.
#[derive(Debug)]
pub(crate) enum Booted {
    Normal,
    Candidate(Box<Candidate>),
    /// The app must end now.
    Exit,
}

/// Looks at the store as the app starts and does what [`boot`] says.
/// `start_watcher` starts a watcher for a trial that has none.
pub(crate) fn boot_app(
    store: &Store,
    me: &str,
    start_watcher: impl Fn(&Store, &Trial) -> io::Result<()>,
) -> Booted {
    boot_app_within(store, me, CANDIDATE_WAIT, &start_watcher)
}

fn boot_app_within(
    store: &Store,
    me: &str,
    candidate_wait: Duration,
    start_watcher: &impl Fn(&Store, &Trial) -> io::Result<()>,
) -> Booted {
    // A decision written by someone else while this one looks is followed on
    // the next look.
    for _ in 0..3 {
        let trial = match store.find() {
            Found::Missing => return Booted::Normal,
            Found::Open(trial) => trial,
            Found::Foreign(protocol) => {
                store.note(&format!("{} leaves a trial of protocol {}", me, protocol));
                return Booted::Normal;
            }
            Found::Damaged(e) => {
                if !store.watcher_alive() && !store.held(FORMER_LOCK) {
                    store.note(&format!("{} discards a damaged trial: {}", me, e));
                    let _ = store.close();
                }
                return Booted::Normal;
            }
            Found::Unreadable(e) => {
                store.note(&format!("{} cannot read the trial: {}", me, e));
                return Booted::Normal;
            }
        };
        let handed_over = store.has(HANDOVER);
        let former_alive = !handed_over && store.held(FORMER_LOCK);
        let watcher_alive = store.watcher_alive();
        let decision = store.decision(&trial.id);
        match boot(
            &trial,
            decision,
            me,
            former_alive,
            handed_over,
            watcher_alive,
        ) {
            Boot::Normal => return Booted::Normal,
            Boot::Candidate => {
                return match take_part(store, &trial, candidate_wait, start_watcher) {
                    Ok(TookPart::Yes(candidate)) => Booted::Candidate(candidate),
                    // Another copy of the new version holds the lock: this one
                    // is a second copy and hands over to it.
                    Ok(TookPart::Busy) => Booted::Normal,
                    Ok(TookPart::Decided) => continue,
                    Err(e) => {
                        store.note(&format!("{} could not take part: {}", me, e));
                        Booted::Normal
                    }
                };
            }
            Boot::HandToWatcher => {
                // Someone is starting the app: whatever is put back is started
                // again, even after a quit.
                let _ = store.unmark(QUIT);
                if watcher_alive {
                    store.note(&format!("{} leaves the trial to its watcher", me));
                    return Booted::Exit;
                }
                return match start_watcher(store, &trial) {
                    Ok(()) => {
                        store.note(&format!("{} started a watcher to finish the trial", me));
                        Booted::Exit
                    }
                    Err(e) => {
                        // Nobody can put the old version back; running this
                        // one is better than running nothing.
                        store.note(&format!("{} could not start a watcher: {}", me, e));
                        if let Ok(Some(Decision::Rejected)) =
                            store.decide(&trial.id, Decision::Rejected)
                        {
                            close_trial(store, &trial, true);
                        }
                        Booted::Normal
                    }
                };
            }
            Boot::Close { reject } => {
                let wanted = if reject {
                    Decision::Rejected
                } else {
                    Decision::Abandoned
                };
                match store.decide(&trial.id, wanted) {
                    // Given up on by a watcher meanwhile: look again.
                    Ok(Some(Decision::Rejected)) if !reject => continue,
                    Ok(Some(decided)) => {
                        store.note(&format!(
                            "{} closes a trial that was {}",
                            me,
                            decided.word()
                        ));
                        close_trial(store, &trial, decided == Decision::Rejected);
                    }
                    // Closed by someone else meanwhile.
                    Ok(None) => {}
                    Err(e) => store.note(&format!("{} could not close the trial: {}", me, e)),
                }
                return Booted::Normal;
            }
        }
    }
    Booted::Normal
}

fn close_trial(store: &Store, trial: &Trial, reject: bool) {
    if reject {
        if let Err(e) = store.remember_rejected(&trial.to) {
            store.note(&format!("could not remember {}: {}", trial.to, e));
        }
    }
    if let Err(e) = store.close() {
        store.note(&format!("could not close the trial: {}", e));
    }
}

/// How a start of the new version went about taking part in its trial.
enum TookPart {
    Yes(Box<Candidate>),
    /// Another copy holds the lock.
    Busy,
    /// The trial was decided while this start waited for the lock, or by
    /// this start; look again.
    Decided,
}

fn take_part(
    store: &Store,
    trial: &Trial,
    wait: Duration,
    start_watcher: &impl Fn(&Store, &Trial) -> io::Result<()>,
) -> io::Result<TookPart> {
    let Some(held) = store.take(CANDIDATE_LOCK, wait)? else {
        return Ok(TookPart::Busy);
    };
    // The copy that held the lock may have decided before it let go.
    if store.decision(&trial.id).is_some() {
        return Ok(TookPart::Decided);
    }
    // A quit before a restart is not the person quitting, and neither is it
    // a start that went wrong. Any other start after the first is one: the
    // last ended without saying it started. Without a watcher (the system
    // restarted since, and none could be started) nothing else stops a
    // version that never starts from being tried at every start.
    let quit = store.has(QUIT);
    store.unmark(QUIT)?;
    let starts = store.count(STARTS) + u32::from(!quit);
    store.mark(STARTS, &starts.to_string())?;
    if starts > MAX_STARTS {
        store.note(&format!(
            "{} was started {} times and never started",
            trial.to,
            starts - 1
        ));
        store.decide(&trial.id, Decision::Rejected)?;
        return Ok(TookPart::Decided);
    }
    store.mark(STARTED, &std::process::id().to_string())?;
    if !store.watcher_alive() {
        // The watcher started for this trial is gone (the system restarted
        // since the install, say): this start gets one of its own.
        if let Err(e) = start_watcher(store, trial) {
            store.note(&format!("{} runs without a watcher: {}", trial.to, e));
        }
    }
    store.note(&format!("{} takes part in {}", trial.to, trial.id));
    Ok(TookPart::Yes(Box::new(Candidate {
        store: store.clone(),
        trial: trial.clone(),
        _held: held,
    })))
}

impl Candidate {
    pub(crate) fn version(&self) -> &str {
        &self.trial.to
    }

    /// The new version has started: the trial ends with it kept. Returns the
    /// decision that stands, which is `Rejected` when the watcher gave up on
    /// it first, or `None` when the trial was closed meanwhile.
    pub(crate) fn confirm(&self) -> io::Result<Option<Decision>> {
        let decided = self.store.decide(&self.trial.id, Decision::Confirmed)?;
        if decided == Some(Decision::Confirmed) {
            self.store.note(&format!("{} confirmed", self.trial.to));
            self.store.forget_rejected(&self.trial.to)?;
            self.store.close()?;
        }
        Ok(decided)
    }

    /// The person quit before the new version had started.
    pub(crate) fn quitting(&self) -> io::Result<()> {
        self.store.mark(QUIT, &self.trial.to)
    }
}

/// What a watcher saw in one look.
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    decision: Option<Decision>,
    /// The old version still runs and has not handed over.
    former_alive: bool,
    installed: bool,
    handed_over: bool,
    /// Since the old version was last seen alive (or since the watcher began).
    since_former: Duration,
    /// Since the new version was first seen started.
    since_started: Option<Duration>,
    /// Since the new version was first seen gone after starting.
    since_gone: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Next {
    Wait,
    /// Nothing more for the watcher to do.
    Leave,
    /// Give up on the new version and put the old one back.
    Reject,
    /// Someone else gave up on it; put the old one back.
    RollBack,
}

fn next(seen: &Seen, limits: &Limits) -> Next {
    match seen.decision {
        Some(Decision::Confirmed) | Some(Decision::Abandoned) => return Next::Leave,
        Some(Decision::Rejected) => return Next::RollBack,
        None => {}
    }
    match (seen.since_started, seen.since_gone) {
        (None, _) if seen.former_alive => Next::Wait,
        (None, _) if seen.handed_over => {
            if seen.since_former >= limits.start {
                Next::Reject
            } else {
                Next::Wait
            }
        }
        // The old version ended after the install without starting the new
        // one; the new one gets its trial when it is next started.
        (None, _) if seen.installed => Next::Leave,
        // The old version ended before its install had returned: what is
        // installed may be half of each.
        (None, _) => {
            if seen.since_former >= limits.settle {
                Next::Reject
            } else {
                Next::Wait
            }
        }
        (Some(_), Some(gone)) if gone >= limits.settle => Next::Reject,
        (Some(running), _) if running >= limits.confirm => Next::Reject,
        _ => Next::Wait,
    }
}

/// How a watcher's watch ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Watched {
    /// The trial was decided without it, or there was nothing to watch.
    Left,
    /// The old version is back.
    RolledBack { relaunched: bool },
    /// Putting the old version back failed too often; the trial is closed
    /// with whatever is installed now, which is started.
    GaveUp,
    /// It could not finish; the trial stays open for the next start.
    Failed(String),
}

/// The watcher: follows the trial `id` until it is decided, and puts the old
/// version back when the new one does not start. `launch` starts the old
/// version again; `kill` ends a new version that hangs.
fn watch(
    store: &Store,
    id: &str,
    limits: Limits,
    launch: impl Fn(&Package) -> io::Result<()>,
    kill: impl Fn(u32) -> io::Result<()>,
) -> Watched {
    // The watcher of a trial that just closed may still hold the lock for a
    // moment.
    let _held = match store.take(WATCHER_LOCK, limits.settle) {
        Ok(Some(held)) => held,
        Ok(None) => return Watched::Left,
        Err(e) => return Watched::Failed(format!("cannot take the watcher's lock: {}", e)),
    };
    // Whoever started this one waits for it: a watcher that never got here
    // watches nothing.
    if let Err(e) = store.mark(WATCHING, id) {
        return Watched::Failed(format!("cannot say it watches: {}", e));
    }
    store.note(&format!("watching {}", id));
    let mut former_seen = Instant::now();
    let mut started_at: Option<Instant> = None;
    let mut gone_at: Option<Instant> = None;
    loop {
        let trial = match store.find() {
            Found::Open(trial) if trial.id == id => trial,
            Found::Unreadable(e) => {
                return Watched::Failed(format!("cannot read the trial: {}", e))
            }
            _ => {
                store.note(&format!("{} is closed", id));
                return Watched::Left;
            }
        };
        let now = Instant::now();
        let handed_over = store.has(HANDOVER);
        let former_alive = !handed_over && store.held(FORMER_LOCK);
        if former_alive {
            former_seen = now;
        }
        let started = store.has(STARTED);
        if started && started_at.is_none() {
            started_at = Some(now);
        }
        if started && !store.held(CANDIDATE_LOCK) {
            gone_at.get_or_insert(now);
        } else {
            gone_at = None;
        }
        let seen = Seen {
            decision: store.decision(id),
            former_alive,
            installed: store.has(INSTALLED),
            handed_over,
            since_former: now - former_seen,
            since_started: started_at.map(|t| now - t),
            since_gone: gone_at.map(|t| now - t),
        };
        match next(&seen, &limits) {
            Next::Wait => std::thread::sleep(limits.poll),
            Next::Leave => {
                store.note(&format!("{} needs no watcher: {:?}", id, seen.decision));
                return Watched::Left;
            }
            Next::Reject => match store.decide(id, Decision::Rejected) {
                Ok(Some(Decision::Rejected)) => {
                    store.note(&format!("{} did not start: {:?}", trial.to, seen));
                    return roll_back(store, &trial, &limits, &launch, &kill);
                }
                // Decided first by someone else; the next look follows it.
                Ok(Some(_)) => {}
                Ok(None) => {
                    store.note(&format!("{} is closed", id));
                    return Watched::Left;
                }
                Err(e) => return Watched::Failed(format!("cannot decide: {}", e)),
            },
            Next::RollBack => return roll_back(store, &trial, &limits, &launch, &kill),
        }
    }
}

fn roll_back(
    store: &Store,
    trial: &Trial,
    limits: &Limits,
    launch: &impl Fn(&Package) -> io::Result<()>,
    kill: &impl Fn(u32) -> io::Result<()>,
) -> Watched {
    if store.attempt() > MAX_ATTEMPTS {
        store.note(&format!("gave up putting {} back", trial.from));
        close_trial(store, trial, true);
        if let Err(e) = launch(&trial.package) {
            store.note(&format!("could not start the app: {}", e));
        }
        return Watched::GaveUp;
    }
    if store.held(CANDIDATE_LOCK) {
        let pid = fs::read_to_string(store.path(STARTED))
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok());
        match pid {
            // Only while its lock is held: the process id is then still the
            // new version's.
            Some(pid) => {
                if let Err(e) = kill(pid) {
                    store.note(&format!("could not end {}: {}", pid, e));
                }
            }
            None => store.note("the new version runs but did not say its process"),
        }
        let asked = Instant::now();
        while store.held(CANDIDATE_LOCK) {
            if asked.elapsed() >= limits.kill {
                store.note("the new version did not end");
                return Watched::Failed("the new version did not end".into());
            }
            std::thread::sleep(limits.poll);
        }
    }
    let mut tries = 0;
    while let Err(e) = restore(&trial.package, &store.path(SLOT)) {
        store.note(&format!("putting {} back failed: {}", trial.from, e));
        tries += 1;
        if tries == 3 {
            return Watched::Failed(format!("could not put the old version back: {}", e));
        }
        std::thread::sleep(limits.settle / 5);
    }
    // Read last: someone who starts the app while it is being put back takes
    // the quit away to have it started again.
    let relaunch = !store.has(QUIT);
    close_trial(store, trial, true);
    store.note(&format!("{} is back", trial.from));
    let relaunched = relaunch
        && match launch(&trial.package) {
            Ok(()) => true,
            Err(e) => {
                store.note(&format!("could not start {} again: {}", trial.from, e));
                false
            }
        };
    Watched::RolledBack { relaunched }
}

/// The arguments of a watcher: `--update-watch <dir> <id>`, or `None` for an
/// ordinary start.
pub(crate) fn watcher_args(args: impl IntoIterator<Item = OsString>) -> Option<(Store, String)> {
    let mut args = args.into_iter().skip(1);
    if args.next()? != OsStr::new(WATCH_ARG) {
        return None;
    }
    let dir = PathBuf::from(args.next()?);
    let id = args.next()?.into_string().ok()?;
    Some((Store::at(dir), id))
}

/// Starts the watcher of `trial` as a process of its own that outlives this
/// one.
/// Returns once it holds its lock: a watcher that cannot run (a copy the
/// system will not start, say) counts as one that could not be started.
pub(crate) fn start_watcher(store: &Store, trial: &Trial) -> io::Result<()> {
    store.unmark(WATCHING)?;
    spawn_detached(
        &watcher_program(store, trial),
        &[
            OsStr::new(WATCH_ARG),
            store.dir.as_os_str(),
            OsStr::new(&trial.id),
        ],
    )?;
    store.watched_by(&trial.id, WATCHER_UP)
}

/// Starts the app of `package` again, as the person would.
fn launch(package: &Package) -> io::Result<()> {
    let target = package.root.join(&package.launch);
    #[cfg(target_os = "macos")]
    return spawn_detached(
        Path::new("/usr/bin/open"),
        &[OsStr::new("-n"), target.as_os_str()],
    );
    #[cfg(not(target_os = "macos"))]
    spawn_detached(&target, &[])
}

fn spawn_detached(program: &Path, args: &[&OsStr]) -> io::Result<()> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Out of this process's group: what ends the app's group does not
        // end what is started here.
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    let mut child = command.spawn()?;
    // Waited for here so that it does not stay behind as a zombie while this
    // process runs.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Ends the process `pid` at once.
fn kill(pid: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        let pid = i32::try_from(pid).map_err(io::Error::other)?;
        // SAFETY: `kill` only sends a signal; a wrong id fails with an error.
        if unsafe { libc::kill(pid, libc::SIGKILL) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
        // SAFETY: the handle is opened, used and closed here.
        unsafe {
            let process = OpenProcess(PROCESS_TERMINATE, false, pid).map_err(io::Error::other)?;
            let ended = TerminateProcess(process, 1).map_err(io::Error::other);
            let _ = CloseHandle(process);
            ended
        }
    }
}

/// The watcher process: runs [`watch`] and ends. Returns the exit code.
pub(crate) fn run_watcher(store: Store, id: String) -> i32 {
    let outcome = watch(&store, &id, Limits::RELEASE, launch, kill);
    store.note(&format!("{} done: {:?}", id, outcome));
    match outcome {
        Watched::Failed(_) => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;

    const A: &str = "0.1.0";
    const B: &str = "0.2.0";

    const SHORT: Limits = Limits {
        start: Duration::from_millis(400),
        confirm: Duration::from_millis(400),
        settle: Duration::from_millis(150),
        kill: Duration::from_millis(300),
        poll: Duration::from_millis(10),
    };

    /// A store and an installed package in a directory of their own.
    struct Fixture {
        _temp: tempfile::TempDir,
        store: Store,
        package: Package,
    }

    impl Fixture {
        /// Laid out as an AppImage: one file.
        fn file() -> Fixture {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("Applications");
            fs::create_dir_all(&root).unwrap();
            let fixture = Fixture {
                store: Store::at(temp.path().join("store")),
                package: Package {
                    root,
                    entries: vec!["jamuru.AppImage".into()],
                    launch: "jamuru.AppImage".into(),
                    program: "jamuru.AppImage".into(),
                },
                _temp: temp,
            };
            fixture.write_version(A);
            fixture
        }

        /// Laid out as an `.app`: a directory with the program inside, a link
        /// and a file that cannot be written to.
        fn bundle() -> Fixture {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("Applications");
            fs::create_dir_all(root.join("jamuru.app/Contents/MacOS")).unwrap();
            #[cfg(unix)]
            std::os::unix::fs::symlink("MacOS", root.join("jamuru.app/Contents/Current")).unwrap();
            let fixture = Fixture {
                store: Store::at(temp.path().join("store")),
                package: Package {
                    root,
                    entries: vec!["jamuru.app".into()],
                    launch: "jamuru.app".into(),
                    program: "jamuru.app/Contents/MacOS/jamuru-app".into(),
                },
                _temp: temp,
            };
            fixture.write_version(A);
            fixture
        }

        /// Laid out as the install directory on Windows: the program, a
        /// directory and the uninstaller, among the app's data.
        fn install_dir() -> Fixture {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("jamuru");
            fs::create_dir_all(root.join("resources")).unwrap();
            fs::create_dir_all(root.join("data")).unwrap();
            fs::write(root.join("data/config.json"), "the person's settings").unwrap();
            let fixture = Fixture {
                store: Store::at(temp.path().join("store")),
                package: Package {
                    root,
                    entries: vec![
                        "jamuru-app.exe".into(),
                        "resources".into(),
                        "uninstall.exe".into(),
                    ],
                    launch: "jamuru-app.exe".into(),
                    program: "jamuru-app.exe".into(),
                },
                _temp: temp,
            };
            fixture.write_version(A);
            fixture
        }

        /// Writes `version` into every file of the package, as an install
        /// would. A version other than A also adds a file A does not have.
        fn write_version(&self, version: &str) {
            let root = &self.package.root;
            match self.package.entries[0].as_str() {
                "jamuru.AppImage" => fs::write(root.join("jamuru.AppImage"), version).unwrap(),
                "jamuru.app" => {
                    let program = root.join(&self.package.program);
                    if program.exists() {
                        let mut permissions = fs::metadata(&program).unwrap().permissions();
                        #[allow(clippy::permissions_set_readonly_false)]
                        permissions.set_readonly(false);
                        fs::set_permissions(&program, permissions).unwrap();
                    }
                    fs::write(&program, version).unwrap();
                    fs::write(root.join("jamuru.app/Contents/Info.plist"), version).unwrap();
                    if version == A {
                        let mut permissions = fs::metadata(&program).unwrap().permissions();
                        permissions.set_readonly(true);
                        fs::set_permissions(&program, permissions).unwrap();
                    } else {
                        fs::create_dir_all(root.join("jamuru.app/Contents/Resources")).unwrap();
                        fs::write(root.join("jamuru.app/Contents/Resources/new"), version).unwrap();
                    }
                }
                _ => {
                    fs::write(root.join("jamuru-app.exe"), version).unwrap();
                    fs::write(root.join("resources/LICENSES.txt"), version).unwrap();
                    fs::write(root.join("uninstall.exe"), version).unwrap();
                    if version != A {
                        fs::write(root.join("resources/new"), version).unwrap();
                    }
                }
            }
        }

        /// The version the program says it is.
        fn version(&self) -> String {
            fs::read_to_string(self.package.root.join(&self.package.program)).unwrap()
        }

        /// Every path under the root with what it is: a listing two trees
        /// can be compared by.
        fn listing(&self) -> Vec<String> {
            fn walk(base: &Path, path: &Path, out: &mut Vec<String>) {
                let meta = fs::symlink_metadata(path).unwrap();
                let name = path.strip_prefix(base).unwrap().display().to_string();
                if meta.file_type().is_symlink() {
                    out.push(format!(
                        "{} -> {}",
                        name,
                        fs::read_link(path).unwrap().display()
                    ));
                } else if meta.is_dir() {
                    out.push(format!("{}/", name));
                    let mut entries: Vec<_> = fs::read_dir(path)
                        .unwrap()
                        .map(|e| e.unwrap().path())
                        .collect();
                    entries.sort();
                    for entry in entries {
                        walk(base, &entry, out);
                    }
                } else {
                    out.push(format!(
                        "{} = {} (read-only: {})",
                        name,
                        fs::read_to_string(path).unwrap(),
                        meta.permissions().readonly()
                    ));
                }
            }
            let mut out = Vec::new();
            walk(&self.package.root, &self.package.root, &mut out);
            out
        }

        fn open(&self) -> Opened {
            open(&self.store, self.package.clone(), A, B, |_, _| Ok(())).unwrap()
        }

        fn trial(&self) -> Option<Trial> {
            match self.store.find() {
                Found::Open(trial) => Some(trial),
                _ => None,
            }
        }
    }

    /// A watcher run on a thread, with the processes it would end and start
    /// stood in for: `kill` lets go of the candidate's lock as its process
    /// ending would.
    struct Watching {
        thread: JoinHandle<Watched>,
        launched: Arc<AtomicUsize>,
        killed: Arc<Mutex<Vec<u32>>>,
    }

    type Slot = Arc<Mutex<Option<Candidate>>>;

    fn watching(fixture: &Fixture, id: &str, candidate: Slot, killable: bool) -> Watching {
        let store = fixture.store.clone();
        let id = id.to_string();
        let launched = Arc::new(AtomicUsize::new(0));
        let killed = Arc::new(Mutex::new(Vec::new()));
        let (counted, recorded) = (launched.clone(), killed.clone());
        let thread = std::thread::spawn(move || {
            watch(
                &store,
                &id,
                SHORT,
                |_| {
                    counted.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                },
                |pid| {
                    recorded.lock().unwrap().push(pid);
                    if killable {
                        candidate.lock().unwrap().take();
                    }
                    Ok(())
                },
            )
        });
        let watching = Watching {
            thread,
            launched,
            killed,
        };
        // Until it holds its lock, a start would start another.
        let began = Instant::now();
        while !fixture.store.watcher_alive() && began.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(5));
        }
        watching
    }

    impl Watching {
        fn end(self) -> (Watched, usize, Vec<u32>) {
            let outcome = self.thread.join().unwrap();
            let killed = self.killed.lock().unwrap().clone();
            (outcome, self.launched.load(Ordering::SeqCst), killed)
        }
    }

    fn no_watcher(_: &Store, _: &Trial) -> io::Result<()> {
        Ok(())
    }

    fn start_as(fixture: &Fixture, version: &str) -> Booted {
        boot_app_within(
            &fixture.store,
            version,
            Duration::from_millis(50),
            &no_watcher,
        )
    }

    fn candidate(booted: Booted) -> Candidate {
        match booted {
            Booted::Candidate(candidate) => *candidate,
            other => panic!("not a candidate: {:?}", other),
        }
    }

    /// The old version installs B and restarts into it.
    fn install_and_hand_over(fixture: &Fixture) -> String {
        let mut opened = fixture.open();
        let id = opened.trial.id.clone();
        fixture.write_version(B);
        opened.installed().unwrap();
        opened.hand_over().unwrap();
        id
    }

    // --- the package ---

    /// Verifies: REQ-UPD-026
    #[test]
    fn when_running_from_an_appimage_the_package_is_the_appimage_file() {
        let package = Package::locate(
            "linux",
            Path::new("/tmp/.mount_jamuruX/usr/bin/jamuru-app"),
            Some(Path::new(
                "/home/u/Applications/jamuru_0.1.0_amd64.AppImage",
            )),
            |_| false,
        )
        .unwrap();
        assert_eq!(package.root, Path::new("/home/u/Applications"));
        assert_eq!(package.entries, vec!["jamuru_0.1.0_amd64.AppImage"]);
        assert_eq!(package.program, Path::new("jamuru_0.1.0_amd64.AppImage"));
        assert_eq!(package.launch, package.program);
    }

    /// Verifies: REQ-UPD-026
    #[test]
    fn when_not_running_from_an_appimage_on_linux_there_is_no_package_to_keep() {
        assert!(
            Package::locate("linux", Path::new("/usr/bin/jamuru-app"), None, |_| false).is_err()
        );
    }

    /// Verifies: REQ-UPD-026
    #[test]
    fn when_running_from_an_app_bundle_the_package_is_the_bundle() {
        let package = Package::locate(
            "macos",
            Path::new("/Applications/jamuru.app/Contents/MacOS/jamuru-app"),
            None,
            |_| false,
        )
        .unwrap();
        assert_eq!(package.root, Path::new("/Applications"));
        assert_eq!(package.entries, vec!["jamuru.app"]);
        assert_eq!(package.launch, Path::new("jamuru.app"));
        assert_eq!(
            package.program,
            Path::new("jamuru.app/Contents/MacOS/jamuru-app")
        );
    }

    /// Verifies: REQ-UPD-026
    #[test]
    fn when_not_running_from_an_app_bundle_on_macos_there_is_no_package_to_keep() {
        assert!(Package::locate(
            "macos",
            Path::new("/usr/local/bin/jamuru-app"),
            None,
            |_| false
        )
        .is_err());
    }

    /// Verifies: REQ-UPD-026
    #[test]
    fn on_windows_the_package_is_what_the_installer_puts_in_the_directory_and_not_the_data() {
        let root = Path::new("C:/Users/u/AppData/Local/jamuru");
        let package = Package::locate("windows", &root.join("jamuru-app.exe"), None, |p| {
            p == root.join("resources") || p == root.join("uninstall.exe")
        })
        .unwrap();
        assert_eq!(package.root, root);
        assert_eq!(
            package.entries,
            vec!["jamuru-app.exe", "resources", "uninstall.exe"]
        );
        let bare =
            Package::locate("windows", &root.join("jamuru-app.exe"), None, |_| false).unwrap();
        assert_eq!(bare.entries, vec!["jamuru-app.exe"]);
    }

    // --- opening a trial ---

    /// Verifies: REQ-UPD-026
    #[test]
    fn opening_a_trial_keeps_a_copy_of_every_entry_and_holds_the_old_versions_lock() {
        for fixture in [Fixture::file(), Fixture::bundle(), Fixture::install_dir()] {
            let before = fixture.listing();
            let mut opened = fixture.open();
            let trial = fixture.trial().unwrap();
            assert_eq!((trial.from.as_str(), trial.to.as_str()), (A, B));
            assert_eq!(trial.package, fixture.package);
            for entry in &fixture.package.entries {
                assert!(fixture.store.path(SLOT).join(entry).exists());
            }
            assert!(!fixture.store.has(SLOT_PARTIAL));
            assert_eq!(
                fixture.listing(),
                before,
                "the package itself is not touched"
            );
            assert!(fixture.store.held(FORMER_LOCK));
            opened.hand_over().unwrap();
            assert!(!fixture.store.held(FORMER_LOCK));
            assert!(fixture.store.has(HANDOVER));
        }
    }

    /// Verifies: REQ-UPD-029
    #[test]
    fn a_trial_is_not_opened_over_another() {
        let fixture = Fixture::file();
        let _opened = fixture.open();
        let error = open(
            &fixture.store,
            fixture.package.clone(),
            A,
            "0.3.0",
            no_watcher,
        )
        .unwrap_err();
        assert!(error.to_string().contains("still being tried"), "{}", error);
    }

    /// Verifies: REQ-UPD-029
    #[test]
    fn a_trial_is_not_opened_over_one_nobody_can_read() {
        let fixture = Fixture::file();
        fs::create_dir_all(&fixture.store.dir).unwrap();
        fs::write(fixture.store.path(TRIAL), r#"{"protocol": 7}"#).unwrap();
        assert!(open(&fixture.store, fixture.package.clone(), A, B, no_watcher).is_err());
    }

    /// Verifies: REQ-UPD-026
    #[test]
    fn a_trial_of_the_running_version_is_not_opened() {
        let fixture = Fixture::file();
        assert!(open(&fixture.store, fixture.package.clone(), A, A, no_watcher).is_err());
        assert!(fixture.trial().is_none());
    }

    /// Verifies: REQ-UPD-026
    #[cfg(unix)]
    #[test]
    fn where_the_app_cannot_write_no_trial_is_opened() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::file();
        fs::set_permissions(&fixture.package.root, fs::Permissions::from_mode(0o555)).unwrap();
        let result = open(&fixture.store, fixture.package.clone(), A, B, no_watcher);
        fs::set_permissions(&fixture.package.root, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(result.is_err());
        assert!(fixture.trial().is_none());
        assert!(fixture.package.writable().is_ok());
    }

    /// The probe beside the package passes, but the package itself is the
    /// person's to write and not the app's: the updater would ask for a
    /// password, and the watcher has none to put it back with.
    ///
    /// Verifies: REQ-UPD-026
    #[cfg(unix)]
    #[test]
    fn where_the_app_cannot_write_the_package_itself_no_trial_is_opened() {
        use std::os::unix::fs::PermissionsExt;
        // SAFETY: only reads the process's own user id.
        if unsafe { libc::geteuid() } == 0 {
            // The superuser may write anything; there is nothing to refuse.
            return;
        }
        let fixture = Fixture::bundle();
        let app = fixture.package.root.join("jamuru.app");
        fs::set_permissions(&app, fs::Permissions::from_mode(0o555)).unwrap();
        let result = open(&fixture.store, fixture.package.clone(), A, B, no_watcher);
        fs::set_permissions(&app, fs::Permissions::from_mode(0o755)).unwrap();
        let error = result.unwrap_err();
        assert!(error.to_string().contains("jamuru.app"), "{}", error);
        assert!(fixture.trial().is_none());
    }

    /// Verifies: REQ-UPD-026
    #[test]
    fn a_watcher_that_runs_but_never_watches_counts_as_not_started() {
        let fixture = Fixture::file();
        let exe = std::env::current_exe().unwrap();
        let result = open(
            &fixture.store,
            fixture.package.clone(),
            A,
            B,
            |store, trial| {
                let args = [OsStr::new("--exact"), OsStr::new("no::such::test")];
                spawn_detached(&exe, &args)?;
                store.watched_by(&trial.id, Duration::from_millis(500))
            },
        );
        let error = result.unwrap_err();
        assert!(error.to_string().contains("did not start"), "{}", error);
        assert!(fixture.trial().is_none());
        assert!(!fixture.store.has(SLOT));
        assert!(!fixture.store.held(FORMER_LOCK));
    }

    /// Verifies: REQ-UPD-026
    #[test]
    fn when_the_watcher_cannot_be_started_the_trial_is_closed_and_not_installed() {
        let fixture = Fixture::file();
        let result = open(&fixture.store, fixture.package.clone(), A, B, |_, _| {
            Err(io::Error::other("no watcher"))
        });
        assert!(result.is_err());
        assert!(fixture.trial().is_none());
        assert!(!fixture.store.has(SLOT));
        assert!(!fixture.store.held(FORMER_LOCK));
    }

    /// Verifies: REQ-UPD-030
    #[test]
    fn when_the_install_fails_the_kept_version_is_put_back_and_the_trial_closed() {
        for fixture in [Fixture::file(), Fixture::bundle(), Fixture::install_dir()] {
            let before = fixture.listing();
            let mut opened = fixture.open();
            fixture.write_version(B);
            opened.abandon(true).unwrap();
            assert_eq!(fixture.listing(), before);
            assert!(fixture.trial().is_none());
            assert!(!fixture.store.has(SLOT));
            assert!(
                !fixture.store.is_rejected(B),
                "an install that failed is not a version that failed"
            );
        }
    }

    // --- putting back ---

    /// Verifies: REQ-UPD-028
    #[test]
    fn putting_back_restores_files_directories_links_and_permissions_and_drops_what_the_update_added(
    ) {
        for fixture in [Fixture::file(), Fixture::bundle(), Fixture::install_dir()] {
            let before = fixture.listing();
            let _opened = fixture.open();
            fixture.write_version(B);
            assert_ne!(fixture.listing(), before);
            restore(&fixture.package, &fixture.store.path(SLOT)).unwrap();
            assert_eq!(fixture.listing(), before);
        }
    }

    /// Verifies: REQ-UPD-028
    #[test]
    fn putting_back_leaves_the_apps_data_beside_the_package_alone() {
        let fixture = Fixture::install_dir();
        let _opened = fixture.open();
        fixture.write_version(B);
        fs::write(
            fixture.package.root.join("data/config.json"),
            "changed by B",
        )
        .unwrap();
        restore(&fixture.package, &fixture.store.path(SLOT)).unwrap();
        assert_eq!(
            fs::read_to_string(fixture.package.root.join("data/config.json")).unwrap(),
            "changed by B"
        );
    }

    /// Verifies: REQ-UPD-028
    #[test]
    fn putting_back_works_when_the_update_removed_an_entry_or_made_it_another_kind() {
        let fixture = Fixture::install_dir();
        let before = fixture.listing();
        let _opened = fixture.open();
        fs::remove_dir_all(fixture.package.root.join("resources")).unwrap();
        fs::write(fixture.package.root.join("resources"), "a file now").unwrap();
        fs::remove_file(fixture.package.root.join("uninstall.exe")).unwrap();
        restore(&fixture.package, &fixture.store.path(SLOT)).unwrap();
        assert_eq!(fixture.listing(), before);
    }

    // --- deciding ---

    /// Verifies: REQ-UPD-027
    #[test]
    fn the_first_decision_written_stands() {
        let fixture = Fixture::file();
        let opened = fixture.open();
        let id = &opened.trial.id;
        assert_eq!(fixture.store.decision(id), None);
        assert_eq!(
            fixture.store.decide(id, Decision::Confirmed).unwrap(),
            Some(Decision::Confirmed)
        );
        assert_eq!(
            fixture.store.decide(id, Decision::Rejected).unwrap(),
            Some(Decision::Confirmed)
        );
        assert_eq!(fixture.store.decision(id), Some(Decision::Confirmed));
    }

    /// Whoever comes late to a trial that was decided and closed must not
    /// decide it again: a watcher would otherwise put back over the version
    /// that was kept.
    ///
    /// Verifies: REQ-UPD-027
    #[test]
    fn a_trial_closed_is_not_decided_again_and_another_trials_decision_is_not_its_own() {
        let fixture = Fixture::file();
        let id = fixture.open().trial.id.clone();
        assert_eq!(
            fixture.store.decide("another", Decision::Rejected).unwrap(),
            None
        );
        assert_eq!(fixture.store.decision(&id), None);
        fixture.store.decide(&id, Decision::Confirmed).unwrap();
        fixture.store.close().unwrap();
        assert_eq!(fixture.store.decision(&id), None);
        assert_eq!(fixture.store.decide(&id, Decision::Rejected).unwrap(), None);
        assert_eq!(fixture.store.decision(&id), None);
        let later = fixture.open();
        assert_eq!(fixture.store.decision(&later.trial.id), None);
    }

    /// Verifies: REQ-UPD-027
    #[test]
    fn of_decisions_written_at_once_exactly_one_stands_for_everyone() {
        for _ in 0..20 {
            let fixture = Fixture::file();
            let opened = fixture.open();
            let barrier = Arc::new(std::sync::Barrier::new(6));
            let threads: Vec<_> = (0..6)
                .map(|i| {
                    let store = fixture.store.clone();
                    let barrier = barrier.clone();
                    let id = opened.trial.id.clone();
                    std::thread::spawn(move || {
                        barrier.wait();
                        let wanted = if i % 2 == 0 {
                            Decision::Confirmed
                        } else {
                            Decision::Rejected
                        };
                        store.decide(&id, wanted).unwrap()
                    })
                })
                .collect();
            let seen: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
            let standing = fixture.store.decision(&opened.trial.id);
            assert!(standing.is_some());
            assert!(
                seen.iter().all(|d| *d == standing),
                "{:?} vs {:?}",
                seen,
                standing
            );
            let leftovers: Vec<_> = fs::read_dir(&fixture.store.dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|name| name.ends_with(".tmp"))
                .collect();
            assert!(leftovers.is_empty(), "{:?}", leftovers);
        }
    }

    /// Verifies: REQ-UPD-027
    #[test]
    fn a_lock_is_held_while_its_holder_keeps_it_and_free_once_it_lets_go() {
        let fixture = Fixture::file();
        assert!(!fixture.store.held(CANDIDATE_LOCK));
        let held = fixture
            .store
            .take(CANDIDATE_LOCK, Duration::ZERO)
            .unwrap()
            .unwrap();
        assert!(fixture.store.held(CANDIDATE_LOCK));
        assert!(fixture
            .store
            .take(CANDIDATE_LOCK, Duration::from_millis(30))
            .unwrap()
            .is_none());
        drop(held);
        assert!(!fixture.store.held(CANDIDATE_LOCK));
        assert!(fixture
            .store
            .take(CANDIDATE_LOCK, Duration::ZERO)
            .unwrap()
            .is_some());
    }

    /// Verifies: REQ-UPD-027
    #[test]
    fn looking_at_a_lock_does_not_keep_its_holder_from_taking_it() {
        let fixture = Fixture::file();
        drop(fixture.store.take(CANDIDATE_LOCK, Duration::ZERO).unwrap());
        let looking = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let looker = {
            let (store, looking) = (fixture.store.clone(), looking.clone());
            std::thread::spawn(move || {
                while looking.load(Ordering::SeqCst) {
                    let _ = store.held(CANDIDATE_LOCK);
                }
            })
        };
        for _ in 0..50 {
            let held = fixture
                .store
                .take(CANDIDATE_LOCK, Duration::from_secs(1))
                .unwrap();
            assert!(held.is_some());
        }
        looking.store(false, Ordering::SeqCst);
        looker.join().unwrap();
    }

    // --- what a watcher does ---

    fn seen() -> Seen {
        Seen {
            decision: None,
            former_alive: false,
            installed: false,
            handed_over: false,
            since_former: Duration::ZERO,
            since_started: None,
            since_gone: None,
        }
    }

    /// Verifies: REQ-UPD-027
    #[test]
    fn a_watcher_leaves_a_trial_that_is_decided_and_puts_back_one_that_was_given_up_on() {
        for decision in [Decision::Confirmed, Decision::Abandoned] {
            let decided = Seen {
                decision: Some(decision),
                since_started: Some(Duration::from_secs(999)),
                ..seen()
            };
            assert_eq!(next(&decided, &SHORT), Next::Leave);
        }
        let rejected = Seen {
            decision: Some(Decision::Rejected),
            ..seen()
        };
        assert_eq!(next(&rejected, &SHORT), Next::RollBack);
    }

    /// Verifies: REQ-UPD-027
    #[test]
    fn a_watcher_waits_as_long_as_the_old_version_runs() {
        let running = Seen {
            former_alive: true,
            installed: true,
            since_former: Duration::ZERO,
            ..seen()
        };
        assert_eq!(next(&running, &SHORT), Next::Wait);
    }

    /// Verifies: REQ-UPD-028
    #[test]
    fn a_watcher_gives_up_on_a_new_version_that_does_not_start_in_time() {
        let handed = Seen {
            handed_over: true,
            installed: true,
            ..seen()
        };
        let waiting = Seen {
            since_former: SHORT.start - Duration::from_millis(1),
            ..handed.clone()
        };
        assert_eq!(next(&waiting, &SHORT), Next::Wait);
        let late = Seen {
            since_former: SHORT.start,
            ..handed
        };
        assert_eq!(next(&late, &SHORT), Next::Reject);
    }

    /// Verifies: REQ-UPD-028
    #[test]
    fn a_watcher_gives_up_on_a_new_version_that_starts_but_does_not_say_so_in_time() {
        let running = Seen {
            handed_over: true,
            since_started: Some(SHORT.confirm - Duration::from_millis(1)),
            ..seen()
        };
        assert_eq!(next(&running, &SHORT), Next::Wait);
        let hung = Seen {
            since_started: Some(SHORT.confirm),
            ..running
        };
        assert_eq!(next(&hung, &SHORT), Next::Reject);
    }

    /// Verifies: REQ-UPD-028
    #[test]
    fn a_watcher_gives_up_on_a_new_version_that_ended_and_did_not_come_back() {
        let gone = Seen {
            handed_over: true,
            since_started: Some(Duration::from_millis(50)),
            since_gone: Some(SHORT.settle - Duration::from_millis(1)),
            ..seen()
        };
        assert_eq!(
            next(&gone, &SHORT),
            Next::Wait,
            "a restart takes its lock again"
        );
        let ended = Seen {
            since_gone: Some(SHORT.settle),
            ..gone
        };
        assert_eq!(next(&ended, &SHORT), Next::Reject);
    }

    /// Verifies: REQ-UPD-030
    #[test]
    fn a_watcher_puts_back_a_package_whose_install_did_not_return() {
        let died = Seen {
            since_former: SHORT.settle - Duration::from_millis(1),
            ..seen()
        };
        assert_eq!(next(&died, &SHORT), Next::Wait);
        let died = Seen {
            since_former: SHORT.settle,
            ..seen()
        };
        assert_eq!(next(&died, &SHORT), Next::Reject);
    }

    /// Verifies: REQ-UPD-031
    #[test]
    fn a_watcher_leaves_an_installed_version_that_was_not_started_to_its_next_start() {
        let installed = Seen {
            installed: true,
            since_former: Duration::from_secs(999),
            ..seen()
        };
        assert_eq!(next(&installed, &SHORT), Next::Leave);
    }

    // --- what a starting app does ---

    fn a_trial() -> Trial {
        Trial {
            protocol: PROTOCOL,
            id: "t".into(),
            from: A.into(),
            to: B.into(),
            package: Fixture::file().package,
        }
    }

    /// Verifies: REQ-UPD-027
    #[test]
    fn a_start_closes_a_trial_that_was_decided() {
        let trial = a_trial();
        for decision in [Decision::Confirmed, Decision::Abandoned] {
            for me in [A, B, "0.3.0"] {
                assert_eq!(
                    boot(&trial, Some(decision), me, false, true, true),
                    Boot::Close { reject: false }
                );
            }
        }
    }

    /// Verifies: REQ-UPD-028
    #[test]
    fn a_start_of_either_version_during_a_put_back_ends_and_leaves_it_to_the_watcher() {
        let trial = a_trial();
        for me in [A, B] {
            for watcher in [false, true] {
                assert_eq!(
                    boot(&trial, Some(Decision::Rejected), me, false, true, watcher),
                    Boot::HandToWatcher
                );
            }
        }
        assert_eq!(
            boot(&trial, Some(Decision::Rejected), "0.3.0", false, true, true),
            Boot::HandToWatcher
        );
        assert_eq!(
            boot(
                &trial,
                Some(Decision::Rejected),
                "0.3.0",
                false,
                true,
                false
            ),
            Boot::Close { reject: true },
            "another version was installed by hand since"
        );
    }

    /// Verifies: REQ-UPD-027
    #[test]
    fn the_first_start_of_the_new_version_takes_part_in_its_trial() {
        let trial = a_trial();
        assert_eq!(boot(&trial, None, B, false, true, true), Boot::Candidate);
        assert_eq!(boot(&trial, None, B, false, true, false), Boot::Candidate);
        // Installed, and the old version ended without restarting.
        assert_eq!(boot(&trial, None, B, false, false, false), Boot::Candidate);
    }

    /// Verifies: REQ-UPD-027
    #[test]
    fn a_second_copy_started_while_the_old_version_runs_starts_as_usual() {
        let trial = a_trial();
        for me in [A, B, "0.3.0"] {
            assert_eq!(boot(&trial, None, me, true, false, true), Boot::Normal);
        }
    }

    /// Verifies: REQ-UPD-027
    #[test]
    fn the_old_version_started_while_the_new_one_is_tried_ends_and_leaves_it_to_the_watcher() {
        let trial = a_trial();
        assert_eq!(
            boot(&trial, None, A, false, true, true),
            Boot::HandToWatcher
        );
        assert_eq!(
            boot(&trial, None, A, false, false, true),
            Boot::HandToWatcher
        );
    }

    /// The installer did not put the new version in place, or only part of
    /// it, and the watcher is gone with the system: what was kept is put back
    /// rather than the half kept.
    ///
    /// Verifies: REQ-UPD-030
    #[test]
    fn the_old_version_started_after_it_handed_over_with_nobody_watching_has_its_copy_put_back() {
        let trial = a_trial();
        assert_eq!(
            boot(&trial, None, A, false, true, false),
            Boot::HandToWatcher
        );
    }

    /// Verifies: REQ-UPD-031
    #[test]
    fn a_trial_nobody_watches_or_runs_is_closed_by_the_next_start() {
        let trial = a_trial();
        assert_eq!(
            boot(&trial, None, A, false, false, false),
            Boot::Close { reject: false }
        );
        for handed_over in [false, true] {
            assert_eq!(
                boot(&trial, None, "0.3.0", false, handed_over, false),
                Boot::Close { reject: false }
            );
        }
    }

    // --- trials from start to end ---

    /// Verifies: REQ-UPD-027
    #[test]
    fn when_the_new_version_says_it_started_it_stays_installed() {
        let fixture = Fixture::bundle();
        let id = install_and_hand_over(&fixture);
        let slot: Slot = Arc::new(Mutex::new(None));
        let watcher = watching(&fixture, &id, slot.clone(), true);
        let started = candidate(start_as(&fixture, B));
        assert_eq!(started.version(), B);
        assert_eq!(started.confirm().unwrap(), Some(Decision::Confirmed));
        let (outcome, launched, killed) = watcher.end();
        assert_eq!(outcome, Watched::Left);
        assert_eq!((launched, killed.len()), (0, 0));
        assert_eq!(fixture.version(), B);
        assert!(fixture.trial().is_none());
        assert!(!fixture.store.has(SLOT));
        assert!(fixture.store.rejected().is_empty());
        assert!(matches!(start_as(&fixture, B), Booted::Normal));
    }

    /// Verifies: REQ-UPD-028
    #[test]
    fn when_the_new_version_never_starts_the_old_one_is_put_back_and_started() {
        for fixture in [Fixture::file(), Fixture::bundle(), Fixture::install_dir()] {
            let before = fixture.listing();
            let id = install_and_hand_over(&fixture);
            let (outcome, launched, killed) =
                watching(&fixture, &id, Arc::new(Mutex::new(None)), true).end();
            assert_eq!(outcome, Watched::RolledBack { relaunched: true });
            assert_eq!((launched, killed.len()), (1, 0));
            assert_eq!(fixture.listing(), before);
            assert!(fixture.trial().is_none());
            assert!(fixture.store.is_rejected(B));
            assert!(matches!(start_as(&fixture, A), Booted::Normal));
        }
    }

    /// Verifies: REQ-UPD-028
    #[test]
    fn when_the_new_version_ends_before_it_says_it_started_the_old_one_is_put_back_and_started() {
        let fixture = Fixture::file();
        let id = install_and_hand_over(&fixture);
        let slot: Slot = Arc::new(Mutex::new(None));
        let watcher = watching(&fixture, &id, slot.clone(), true);
        drop(candidate(start_as(&fixture, B)));
        let (outcome, launched, killed) = watcher.end();
        assert_eq!(outcome, Watched::RolledBack { relaunched: true });
        assert_eq!((launched, killed.len()), (1, 0));
        assert_eq!(fixture.version(), A);
        assert!(fixture.store.is_rejected(B));
    }

    /// Verifies: REQ-UPD-028
    #[test]
    fn when_the_new_version_hangs_before_it_says_it_started_it_is_ended_and_the_old_one_put_back() {
        let fixture = Fixture::bundle();
        let id = install_and_hand_over(&fixture);
        let slot: Slot = Arc::new(Mutex::new(None));
        let watcher = watching(&fixture, &id, slot.clone(), true);
        *slot.lock().unwrap() = Some(candidate(start_as(&fixture, B)));
        let (outcome, launched, killed) = watcher.end();
        assert_eq!(outcome, Watched::RolledBack { relaunched: true });
        assert_eq!(killed, vec![std::process::id()]);
        assert_eq!(launched, 1);
        assert_eq!(fixture.version(), A);
        assert!(slot.lock().unwrap().is_none());
    }

    /// Verifies: REQ-UPD-028
    #[test]
    fn when_the_person_quits_the_new_version_before_it_started_the_old_one_is_put_back_but_not_started(
    ) {
        let fixture = Fixture::file();
        let id = install_and_hand_over(&fixture);
        let watcher = watching(&fixture, &id, Arc::new(Mutex::new(None)), true);
        let started = candidate(start_as(&fixture, B));
        started.quitting().unwrap();
        drop(started);
        let (outcome, launched, _) = watcher.end();
        assert_eq!(outcome, Watched::RolledBack { relaunched: false });
        assert_eq!(launched, 0);
        assert_eq!(fixture.version(), A);
    }

    /// Verifies: REQ-UPD-028
    #[test]
    fn when_the_new_version_restarts_itself_before_it_says_it_started_its_trial_goes_on() {
        let fixture = Fixture::file();
        let id = install_and_hand_over(&fixture);
        let watcher = watching(&fixture, &id, Arc::new(Mutex::new(None)), true);
        let first = candidate(start_as(&fixture, B));
        first.quitting().unwrap();
        drop(first);
        std::thread::sleep(SHORT.settle / 3);
        let second = candidate(start_as(&fixture, B));
        assert!(
            !fixture.store.has(QUIT),
            "the restarted copy is not quitting"
        );
        assert_eq!(second.confirm().unwrap(), Some(Decision::Confirmed));
        assert_eq!(watcher.end().0, Watched::Left);
        assert_eq!(fixture.version(), B);
    }

    /// Verifies: REQ-UPD-030
    #[test]
    fn when_the_old_version_ends_in_the_middle_of_the_install_the_old_version_is_put_back() {
        for fixture in [Fixture::file(), Fixture::bundle(), Fixture::install_dir()] {
            let before = fixture.listing();
            let opened = fixture.open();
            let id = opened.trial.id.clone();
            let watcher = watching(&fixture, &id, Arc::new(Mutex::new(None)), true);
            fixture.write_version("half of B");
            drop(opened);
            let (outcome, launched, _) = watcher.end();
            assert_eq!(outcome, Watched::RolledBack { relaunched: true });
            assert_eq!(launched, 1);
            assert_eq!(fixture.listing(), before);
        }
    }

    /// Verifies: REQ-UPD-030
    #[test]
    fn when_the_install_fails_the_watcher_leaves() {
        let fixture = Fixture::file();
        let mut opened = fixture.open();
        let id = opened.trial.id.clone();
        let watcher = watching(&fixture, &id, Arc::new(Mutex::new(None)), true);
        fixture.write_version(B);
        opened.abandon(true).unwrap();
        let (outcome, launched, _) = watcher.end();
        assert_eq!(outcome, Watched::Left);
        assert_eq!(launched, 0);
        assert_eq!(fixture.version(), A);
    }

    /// Verifies: REQ-UPD-031
    #[test]
    fn when_the_old_version_ends_after_the_install_without_restarting_the_new_one_is_tried_at_its_next_start(
    ) {
        let fixture = Fixture::file();
        let opened = fixture.open();
        let id = opened.trial.id.clone();
        let watcher = watching(&fixture, &id, Arc::new(Mutex::new(None)), true);
        fixture.write_version(B);
        opened.installed().unwrap();
        drop(opened);
        assert_eq!(watcher.end().0, Watched::Left);
        assert!(fixture.trial().is_some(), "kept for the next start");

        let watchers = AtomicUsize::new(0);
        let booted = boot_app_within(
            &fixture.store,
            B,
            Duration::from_millis(50),
            &|_: &Store, _: &Trial| {
                watchers.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        );
        let started = candidate(booted);
        assert_eq!(
            watchers.load(Ordering::SeqCst),
            1,
            "the start brings its own watcher"
        );
        assert_eq!(
            fs::read_to_string(fixture.store.path(STARTED)).unwrap(),
            std::process::id().to_string()
        );
        assert_eq!(started.confirm().unwrap(), Some(Decision::Confirmed));
    }

    /// Verifies: REQ-UPD-028
    #[test]
    fn when_the_new_version_does_not_end_when_asked_the_trial_stays_for_the_next_start_to_finish() {
        let fixture = Fixture::file();
        let id = install_and_hand_over(&fixture);
        let slot: Slot = Arc::new(Mutex::new(None));
        let watcher = watching(&fixture, &id, slot.clone(), false);
        *slot.lock().unwrap() = Some(candidate(start_as(&fixture, B)));
        let (outcome, _, killed) = watcher.end();
        assert!(matches!(outcome, Watched::Failed(_)), "{:?}", outcome);
        assert_eq!(killed.len(), 1);
        assert_eq!(fixture.store.decision(&id), Some(Decision::Rejected));
        assert_eq!(
            fixture.version(),
            B,
            "nothing is replaced under a running program"
        );

        slot.lock().unwrap().take();
        let watchers = AtomicUsize::new(0);
        let booted = boot_app_within(
            &fixture.store,
            B,
            Duration::from_millis(50),
            &|_: &Store, _: &Trial| {
                watchers.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        );
        assert!(matches!(booted, Booted::Exit));
        assert_eq!(watchers.load(Ordering::SeqCst), 1);
        let (outcome, launched, _) = watching(&fixture, &id, slot, true).end();
        assert_eq!(outcome, Watched::RolledBack { relaunched: true });
        assert_eq!(launched, 1);
        assert_eq!(fixture.version(), A);
    }

    /// Verifies: REQ-UPD-029
    #[test]
    fn when_putting_back_keeps_failing_the_trial_is_given_up_and_the_app_still_starts() {
        let fixture = Fixture::file();
        let id = install_and_hand_over(&fixture);
        fs::remove_file(fixture.store.path(SLOT).join("jamuru.AppImage")).unwrap();
        for _ in 0..MAX_ATTEMPTS {
            let (outcome, launched, _) =
                watching(&fixture, &id, Arc::new(Mutex::new(None)), true).end();
            assert!(matches!(outcome, Watched::Failed(_)), "{:?}", outcome);
            assert_eq!(launched, 0);
            assert!(fixture.trial().is_some());
        }
        let (outcome, launched, _) =
            watching(&fixture, &id, Arc::new(Mutex::new(None)), true).end();
        assert_eq!(outcome, Watched::GaveUp);
        assert_eq!(launched, 1);
        assert!(fixture.trial().is_none());
        assert!(fixture.store.is_rejected(B));
        assert!(matches!(start_as(&fixture, B), Booted::Normal));
    }

    /// Verifies: REQ-UPD-028
    #[test]
    fn a_start_while_the_old_version_is_put_back_has_it_started_again_even_after_a_quit() {
        let fixture = Fixture::file();
        let id = install_and_hand_over(&fixture);
        let started = candidate(start_as(&fixture, B));
        started.quitting().unwrap();
        drop(started);
        fixture.store.decide(&id, Decision::Rejected).unwrap();
        let _watcher = fixture
            .store
            .take(WATCHER_LOCK, Duration::ZERO)
            .unwrap()
            .unwrap();
        assert!(matches!(start_as(&fixture, A), Booted::Exit));
        assert!(!fixture.store.has(QUIT));
    }

    /// Verifies: REQ-UPD-027
    #[test]
    fn when_the_new_version_says_it_started_after_the_watcher_gave_up_the_watcher_wins() {
        let fixture = Fixture::file();
        let id = install_and_hand_over(&fixture);
        let started = candidate(start_as(&fixture, B));
        fixture.store.decide(&id, Decision::Rejected).unwrap();
        assert_eq!(started.confirm().unwrap(), Some(Decision::Rejected));
        assert!(
            fixture.trial().is_some(),
            "left for the watcher to put back"
        );
    }

    /// Verifies: REQ-UPD-027
    #[test]
    fn a_second_copy_of_the_new_version_does_not_take_part_in_the_trial() {
        let fixture = Fixture::file();
        install_and_hand_over(&fixture);
        let _first = candidate(start_as(&fixture, B));
        assert!(matches!(start_as(&fixture, B), Booted::Normal));
    }

    /// A start that waited for the lock of a copy that was given up on
    /// meanwhile must not take part in a trial that is over: it would mark
    /// itself started and be ended by the watcher putting the old one back.
    ///
    /// Verifies: REQ-UPD-027
    #[test]
    fn a_start_that_waited_for_the_lock_follows_what_was_decided_meanwhile() {
        let fixture = Fixture::file();
        let id = install_and_hand_over(&fixture);
        let held = fixture
            .store
            .take(CANDIDATE_LOCK, Duration::ZERO)
            .unwrap()
            .unwrap();
        let deciding = {
            let (store, id) = (fixture.store.clone(), id.clone());
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                store.decide(&id, Decision::Rejected).unwrap();
                drop(held);
            })
        };
        let booted = boot_app_within(&fixture.store, B, Duration::from_secs(5), &no_watcher);
        deciding.join().unwrap();
        assert!(matches!(booted, Booted::Exit), "{:?}", booted);
        assert!(!fixture.store.has(STARTED));
        assert_eq!(fixture.store.decision(&id), Some(Decision::Rejected));
    }

    /// Without a watcher (none could be started since the system restarted)
    /// nothing else ends the trial of a version that never starts.
    ///
    /// Verifies: REQ-UPD-028
    #[test]
    fn a_new_version_that_keeps_ending_before_it_starts_is_given_up_on_after_a_few_starts() {
        let fixture = Fixture::file();
        let id = install_and_hand_over(&fixture);
        for _ in 0..5 {
            let started = candidate(start_as(&fixture, B));
            started.quitting().unwrap();
        }
        assert_eq!(
            fixture.store.count(STARTS),
            1,
            "a restart after a quit is not a start that went wrong"
        );
        // The first of these follows the last quit, and is a restart too.
        for _ in 0..MAX_STARTS {
            drop(candidate(start_as(&fixture, B)));
        }
        assert_eq!(fixture.store.count(STARTS), MAX_STARTS);
        let no_watcher_at_all = |_: &Store, _: &Trial| Err(io::Error::other("cannot start"));
        let booted = boot_app_within(
            &fixture.store,
            B,
            Duration::from_millis(50),
            &no_watcher_at_all,
        );
        assert!(matches!(booted, Booted::Normal), "{:?}", booted);
        assert!(fixture.trial().is_none(), "{} is closed", id);
        assert!(fixture.store.is_rejected(B));
    }

    /// Verifies: REQ-UPD-031
    #[test]
    fn a_damaged_trial_nobody_takes_part_in_is_discarded_and_one_of_a_later_protocol_is_left() {
        let fixture = Fixture::file();
        fs::create_dir_all(&fixture.store.dir).unwrap();
        fs::write(fixture.store.path(TRIAL), "{ not json").unwrap();
        assert!(matches!(start_as(&fixture, A), Booted::Normal));
        assert!(!fixture.store.is_open());

        let later = r#"{"protocol": 2, "anything": "else"}"#;
        fs::write(fixture.store.path(TRIAL), later).unwrap();
        assert!(matches!(start_as(&fixture, A), Booted::Normal));
        assert_eq!(
            fs::read_to_string(fixture.store.path(TRIAL)).unwrap(),
            later
        );
    }

    /// Verifies: REQ-UPD-027
    #[test]
    fn a_watcher_of_another_trial_or_of_one_already_watched_leaves() {
        let fixture = Fixture::file();
        let _opened = fixture.open();
        let (outcome, _, _) = watching(&fixture, "another", Arc::new(Mutex::new(None)), true).end();
        assert_eq!(outcome, Watched::Left);

        let id = fixture.trial().unwrap().id;
        let _first = fixture
            .store
            .take(WATCHER_LOCK, Duration::ZERO)
            .unwrap()
            .unwrap();
        let began = Instant::now();
        let outcome = watch(&fixture.store, &id, SHORT, |_| Ok(()), |_| Ok(()));
        assert_eq!(outcome, Watched::Left);
        assert!(began.elapsed() >= SHORT.settle);
    }

    // --- versions given up on ---

    /// Verifies: REQ-UPD-029
    #[test]
    fn versions_given_up_on_are_remembered_the_latest_few_and_forgotten_once_one_starts() {
        let fixture = Fixture::file();
        for i in 0..(REJECTED_KEPT + 3) {
            fixture
                .store
                .remember_rejected(&format!("0.0.{}", i))
                .unwrap();
        }
        let kept = fixture.store.rejected();
        assert_eq!(kept.len(), REJECTED_KEPT);
        assert_eq!(kept.last().unwrap(), &format!("0.0.{}", REJECTED_KEPT + 2));
        assert!(!fixture.store.is_rejected("0.0.0"));
        fixture.store.remember_rejected("0.0.5").unwrap();
        assert_eq!(fixture.store.rejected().last().unwrap(), "0.0.5");
        fixture.store.forget_rejected("0.0.5").unwrap();
        assert!(!fixture.store.is_rejected("0.0.5"));
    }

    /// Verifies: REQ-UPD-029
    #[test]
    fn a_release_tried_by_itself_is_held_back_for_a_while_and_only_that_release() {
        let fixture = Fixture::file();
        let lately = |version| {
            fixture
                .store
                .tried_by_itself_lately(version, Duration::from_secs(60))
        };
        assert!(!lately(B));
        fixture.store.tried_by_itself(B).unwrap();
        assert!(lately(B));
        assert!(!lately("0.3.0"));
        assert!(!fixture.store.tried_by_itself_lately(B, Duration::ZERO));
        fixture.store.tried_by_itself("0.3.0").unwrap();
        assert!(!lately(B), "only the latest is held back");
    }

    // --- the watcher's command line ---

    /// Verifies: REQ-UPD-027
    #[test]
    fn only_a_command_line_that_names_the_watcher_starts_one() {
        let args = |list: &[&str]| list.iter().map(OsString::from).collect::<Vec<_>>();
        let (store, id) =
            watcher_args(args(&["jamuru-app", WATCH_ARG, "/tmp/store", "abc-1"])).unwrap();
        assert_eq!(store.dir, Path::new("/tmp/store"));
        assert_eq!(id, "abc-1");
        assert!(watcher_args(args(&["jamuru-app"])).is_none());
        assert!(watcher_args(args(&["jamuru-app", "jamuru://join/abc"])).is_none());
        assert!(watcher_args(args(&["jamuru-app", WATCH_ARG, "/tmp/store"])).is_none());
    }

    // --- with real processes ---

    const CHILD: &str = "jamuru-trial-child=";
    const CHILD_TEST: &str = "updater::trial::tests::the_process_the_process_tests_start";

    const PROCESS_LIMITS: Limits = Limits {
        start: Duration::from_secs(20),
        confirm: Duration::from_secs(2),
        settle: Duration::from_secs(1),
        kill: Duration::from_secs(5),
        poll: Duration::from_millis(50),
    };

    fn child_args(role: &str, dir: &Path, id: &str) -> Vec<OsString> {
        let mut args: Vec<OsString> = ["--exact", CHILD_TEST, "--ignored", "--test-threads=1"]
            .iter()
            .map(OsString::from)
            .collect();
        let mut arg = OsString::from(CHILD);
        arg.push(format!("{}|{}|", role, id));
        arg.push(dir.as_os_str());
        args.push(arg);
        args
    }

    /// The watcher and the new versions of the tests below, as processes of
    /// their own: started from this test binary with a role, run alone.
    #[test]
    #[ignore = "the body of the processes the tests with real processes start"]
    fn the_process_the_process_tests_start() {
        let Some(arg) = std::env::args().find_map(|a| a.strip_prefix(CHILD).map(str::to_owned))
        else {
            return;
        };
        let mut parts = arg.splitn(3, '|');
        let (role, id, dir) = (
            parts.next().unwrap(),
            parts.next().unwrap(),
            parts.next().unwrap(),
        );
        let store = Store::at(dir);
        match role {
            "watch" => {
                let relaunched = store.path("relaunched");
                let outcome = watch(
                    &store,
                    id,
                    PROCESS_LIMITS,
                    |_| fs::write(&relaunched, "yes"),
                    kill,
                );
                fs::write(store.path("watched"), format!("{:?}", outcome)).unwrap();
            }
            behaviour => {
                let started = candidate(boot_app_within(&store, B, CANDIDATE_WAIT, &no_watcher));
                match behaviour {
                    "hang" => std::thread::sleep(Duration::from_secs(120)),
                    "crash" => std::process::exit(3),
                    "confirm" => {
                        started.confirm().unwrap();
                    }
                    other => panic!("no role {}", other),
                }
            }
        }
    }

    /// Opens a trial whose watcher is a process of its own, installs B and
    /// hands over, then starts B as a process with `behaviour`.
    fn tried_by_processes(fixture: &Fixture, behaviour: &str) -> std::process::ExitStatus {
        let exe = std::env::current_exe().unwrap();
        let mut opened = open(
            &fixture.store,
            fixture.package.clone(),
            A,
            B,
            |store, trial| {
                let args = child_args("watch", &store.dir, &trial.id);
                let args: Vec<&OsStr> = args.iter().map(OsString::as_os_str).collect();
                spawn_detached(&exe, &args)?;
                store.watched_by(&trial.id, Duration::from_secs(10))
            },
        )
        .unwrap();
        let id = opened.trial.id.clone();
        assert!(fixture.store.watcher_alive());
        fixture.write_version(B);
        opened.installed().unwrap();
        opened.hand_over().unwrap();
        drop(opened);
        let status = Command::new(&exe)
            .args(child_args(behaviour, &fixture.store.dir, &id))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        let began = Instant::now();
        while !fixture.store.has("watched") {
            assert!(
                began.elapsed() < Duration::from_secs(20),
                "the watcher did not finish"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        status
    }

    fn watched(fixture: &Fixture) -> String {
        fs::read_to_string(fixture.store.path("watched")).unwrap()
    }

    /// Verifies: REQ-UPD-028
    #[test]
    fn a_new_version_process_that_hangs_is_ended_by_the_watcher_process_and_the_old_one_put_back() {
        let fixture = Fixture::bundle();
        let before = fixture.listing();
        let status = tried_by_processes(&fixture, "hang");
        assert!(!status.success(), "ended from outside: {:?}", status);
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(status.signal(), Some(libc::SIGKILL));
        }
        assert_eq!(watched(&fixture), "RolledBack { relaunched: true }");
        assert!(fixture.store.has("relaunched"));
        assert_eq!(fixture.listing(), before);
        assert!(fixture.store.is_rejected(B));
    }

    /// Verifies: REQ-UPD-028
    #[test]
    fn a_new_version_process_that_crashes_is_noticed_by_the_watcher_process_and_the_old_one_put_back(
    ) {
        let fixture = Fixture::file();
        let status = tried_by_processes(&fixture, "crash");
        assert_eq!(status.code(), Some(3));
        assert_eq!(watched(&fixture), "RolledBack { relaunched: true }");
        assert_eq!(fixture.version(), A);
    }

    /// Verifies: REQ-UPD-027
    #[test]
    fn a_new_version_process_that_says_it_started_is_left_installed_by_the_watcher_process() {
        let fixture = Fixture::file();
        let status = tried_by_processes(&fixture, "confirm");
        assert!(status.success());
        assert_eq!(watched(&fixture), "Left");
        assert!(!fixture.store.has("relaunched"));
        assert_eq!(fixture.version(), B);
        assert!(fixture.trial().is_none());
    }
}
