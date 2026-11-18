use crate::updates::{self, Update};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

const FAILED: &str = "update-failed";
const STARTING: &str = "LANLINK_UPGRADE_START";
const HANDOFF: &str = "--apply-prepared-update";

#[derive(Clone)]
pub struct Updater {
    install: Option<PathBuf>,
    busy: Arc<Mutex<()>>,
    pub shutdown: CancellationToken,
}

#[derive(Serialize, Deserialize)]
struct Job {
    install: PathBuf,
    root: PathBuf,
    cwd: PathBuf,
    origin: String,
    version: String,
    previous: String,
}

impl Updater {
    pub fn new(web_dir: &Path, root: &Path, http_port: u16) -> Self {
        Self {
            install: (http_port != 0)
                .then(|| managed_install(web_dir, root))
                .flatten(),
            busy: Arc::new(Mutex::new(())),
            shutdown: CancellationToken::new(),
        }
    }

    pub fn check(&self) -> Result<Update> {
        if self.install.is_none() {
            return Ok(Update::Unsupported);
        }
        Ok(match updates::latest()? {
            Some(release) => Update::Available {
                version: release.version.to_string(),
            },
            None => Update::Current,
        })
    }

    pub fn prepare(&self, requested: &str, root: PathBuf, origin: String) -> Result<()> {
        let _busy = self
            .busy
            .try_lock()
            .map_err(|_| anyhow::anyhow!("update already in progress"))?;
        ensure!(!self.shutdown.is_cancelled(), "update already handed off");
        let install = self
            .install
            .clone()
            .context("installation does not support updates")?;
        let job = Job {
            install,
            root,
            cwd: std::env::current_dir()?,
            origin,
            version: requested.to_owned(),
            previous: updates::INSTALLED_VERSION.to_owned(),
        };
        let release = updates::latest()?.context("no update available")?;
        ensure!(
            release.version.to_string() == requested,
            "the available update has changed"
        );
        let parent = job.install.parent().context("installation has no parent")?;
        let stage = tempfile::Builder::new()
            .prefix(".lanlink-upgrade-")
            .tempdir_in(parent)?;
        #[cfg(windows)]
        {
            discard_committed_backup(stage.path(), &job.install)?;
            if stage.path().join("retired").is_dir() {
                fs::remove_dir_all(stage.path().join("retired"))?;
            }
        }
        #[cfg(windows)]
        for entry in fs::read_dir(parent)?.flatten() {
            let path = entry.path();
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(".lanlink-upgrade-")
                && entry.file_type().is_ok_and(|kind| kind.is_dir())
                && path.join("finished").is_file()
            {
                let _ = fs::remove_dir_all(path);
            }
        }
        let archive = stage.path().join("package.zip");
        release.download(&mut File::create(&archive)?)?;
        let extracted = stage.path().join("unpacked");
        let payload = extract(&archive, &extracted)?;
        fs::remove_file(&archive)?;
        ensure!(
            payload.join(binary_path()).is_file()
                && payload.join(web_path()).join("index.html").is_file(),
            "release layout is invalid"
        );
        fs::rename(payload, stage.path().join("next"))?;
        if extracted.exists() {
            fs::remove_dir_all(extracted)?;
        }

        match fs::remove_file(job.root.join(FAILED)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let helper = stage
            .path()
            .join(format!("helper{}", std::env::consts::EXE_SUFFIX));
        fs::copy(std::env::current_exe()?, &helper)?;
        serde_json::to_writer(File::create(stage.path().join("job.json"))?, &job)?;
        let mut command = background(&helper, stage.path());
        command
            .arg(HANDOFF)
            .arg(stage.path())
            .args(std::env::args_os().skip(1));
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(job.root.join("logs/update.log"))?;
        let mut child = command.stderr(log).spawn()?;
        if let Err(error) = wait_for(&mut child, Duration::from_secs(5), || {
            stage.path().join("ready").is_file()
        }) {
            terminate(&mut child)?;
            return Err(error);
        }
        let _ = stage.keep();
        self.shutdown.cancel();
        Ok(())
    }
}

pub fn startup_lock(root: &Path) -> Result<Option<File>> {
    if std::env::var_os(STARTING).is_some() {
        return Ok(None);
    }
    let lock = open_lock(root, "update.lock")?;
    fs2::FileExt::try_lock_shared(&lock).context("LANLink update is in progress")?;
    Ok(Some(lock))
}

pub fn failure(root: &Path) -> Option<String> {
    fs::read_to_string(root.join(FAILED))
        .ok()
        .filter(|version| version.len() <= 80)
}

pub fn run_helper() -> Option<Result<()>> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new(HANDOFF)) {
        return None;
    }
    Some((|| {
        let stage = PathBuf::from(args.next().context("missing update stage")?);
        let job: Job = serde_json::from_reader(File::open(stage.join("job.json"))?)?;
        let args: Vec<_> = args.collect();
        complete(&stage, &job, &args)
    })())
}

fn complete(stage: &Path, job: &Job, args: &[OsString]) -> Result<()> {
    let lock = open_lock(&job.root, "update.lock")?;
    fs2::FileExt::try_lock_exclusive(&lock)?;
    File::create(stage.join("ready"))?;
    let agent = open_lock(&job.root, "agent.lock")?;
    if let Err(error) = wait_for_exit(&agent) {
        fs::write(job.root.join(FAILED), &job.version)?;
        return Err(error);
    }
    drop(agent);
    let result = replace(stage, &job.install);
    if let Err(error) = result {
        return restart_previous(stage, job, args, error);
    }
    if let Err(error) = start(job, args, &job.version) {
        restore(stage, &job.install)?;
        return restart_previous(stage, job, args, error);
    }
    #[cfg(windows)]
    {
        File::create(previous(&job.install).join(".committed"))?;
        if let Err(error) = discard_committed_backup(stage, &job.install) {
            eprintln!("Updated LANLink, could not remove the previous version: {error:#}");
        }
    }
    cleanup(stage);
    Ok(())
}

fn restart_previous(
    stage: &Path,
    job: &Job,
    args: &[OsString],
    error: anyhow::Error,
) -> Result<()> {
    let marked = fs::write(job.root.join(FAILED), &job.version);
    start(job, args, &job.previous).context("restored LANLink could not start")?;
    marked?;
    cleanup(stage);
    Err(error)
}

fn cleanup(stage: &Path) {
    #[cfg(windows)]
    {
        // The helper image stays locked until it exits. Only finished stages are eligible for cleanup.
        for entry in ["next", "rejected", "retired"] {
            let _ = fs::remove_dir_all(stage.join(entry));
        }
        let _ = File::create(stage.join("finished"));
    }
    #[cfg(not(windows))]
    let _ = fs::remove_dir_all(stage);
}

#[cfg(windows)]
fn previous(install: &Path) -> PathBuf {
    install.with_file_name(".previous")
}

#[cfg(windows)]
fn discard_committed_backup(stage: &Path, install: &Path) -> Result<()> {
    let backup = previous(install);
    if backup.join(".committed").is_file() {
        move_directory(&backup, &stage.join("retired"))?;
    }
    Ok(())
}

#[cfg(windows)]
fn replace(stage: &Path, install: &Path) -> Result<()> {
    let previous = previous(install);
    move_directory(install, &previous)?;
    if let Err(error) = move_directory(&stage.join("next"), install) {
        move_directory(&previous, install).context("could not restore Windows installation")?;
        return Err(error);
    }
    Ok(())
}

#[cfg(windows)]
fn restore(stage: &Path, install: &Path) -> Result<()> {
    move_directory(install, &stage.join("rejected"))?;
    move_directory(&previous(install), install)
}

#[cfg(target_os = "macos")]
fn replace(stage: &Path, install: &Path) -> Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let old = CString::new(install.as_os_str().as_bytes())?;
    let next = CString::new(stage.join("next").as_os_str().as_bytes())?;
    // Both NUL-terminated paths live across the call and are on the same volume.
    if unsafe { libc::renamex_np(old.as_ptr(), next.as_ptr(), libc::RENAME_SWAP) } != 0 {
        return Err(std::io::Error::last_os_error()).context("could not replace application");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn restore(stage: &Path, install: &Path) -> Result<()> {
    replace(stage, install)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn replace(_: &Path, _: &Path) -> Result<()> {
    anyhow::bail!("unsupported installation")
}

#[cfg(not(any(target_os = "macos", windows)))]
fn restore(stage: &Path, install: &Path) -> Result<()> {
    replace(stage, install)
}

fn open_lock(root: &Path, name: &str) -> Result<File> {
    Ok(OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(name))?)
}

fn wait_for_exit(agent: &File) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        match fs2::FileExt::try_lock_exclusive(agent) {
            Ok(()) => return Ok(()),
            Err(error)
                if error.kind() == fs2::lock_contended_error().kind()
                    && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(error).context("previous LANLink did not stop"),
        }
    }
}

#[cfg(windows)]
fn move_directory(from: &Path, to: &Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(error)
                if cfg!(windows)
                    && error
                        .raw_os_error()
                        .is_some_and(|code| matches!(code, 5 | 32 | 33))
                    && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => {
                return Err(error).with_context(|| format!("could not move {}", from.display()));
            }
        }
    }
}

fn start(job: &Job, args: &[OsString], expected: &str) -> Result<()> {
    let binary = job.install.join(binary_path());
    let mut child = background(&binary, &job.cwd)
        .args(args)
        .env(STARTING, "1")
        .spawn()?;
    let http: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_millis(500)))
        .max_redirects(0)
        .proxy(None)
        .build()
        .into();
    let ready = wait_for(&mut child, Duration::from_secs(12), || {
        http.get(format!("{}/api/v1/health", job.origin))
            .call()
            .ok()
            .and_then(|mut response| {
                response
                    .body_mut()
                    .with_config()
                    .limit(2048)
                    .read_to_vec()
                    .ok()
            })
            .and_then(|body| serde_json::from_slice::<serde_json::Value>(&body).ok())
            .is_some_and(|health| {
                health["service"] == "lanlink"
                    && health["status"] == "ok"
                    && health["version"] == expected
            })
    });
    if ready.is_err() {
        terminate(&mut child)?;
    }
    ready
}

fn wait_for(child: &mut Child, timeout: Duration, mut ready: impl FnMut() -> bool) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        ensure!(
            child.try_wait()?.is_none(),
            "LANLink process stopped before it was ready"
        );
        if ready() && child.try_wait()?.is_none() {
            return Ok(());
        }
        ensure!(
            Instant::now() < deadline,
            "LANLink process did not become ready"
        );
        thread::sleep(Duration::from_millis(100));
    }
}

fn terminate(child: &mut Child) -> Result<()> {
    if child.try_wait()?.is_none() {
        child.kill()?;
        child.wait()?;
    }
    Ok(())
}

fn background(binary: &Path, cwd: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

fn managed_install(web_dir: &Path, root: &Path) -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?.canonicalize().ok()?;
    let path = if cfg!(target_os = "macos") {
        let bundle = executable.ancestors().nth(3)?;
        if bundle.file_name()? != "LANLink.app" || !bundle.join("Contents/Info.plist").is_file() {
            return None;
        }
        bundle.to_path_buf()
    } else if cfg!(windows) {
        PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Programs/LANLink/current")
    } else {
        return None;
    };
    let install = path.canonicalize().ok()?;
    if executable == install.join(binary_path())
        && web_dir.canonicalize().ok()? == install.join(web_path())
        && !root.starts_with(&install)
    {
        Some(path)
    } else {
        None
    }
}

fn binary_path() -> &'static str {
    if cfg!(target_os = "macos") {
        "Contents/MacOS/lanlink-agent"
    } else {
        "lanlink-agent.exe"
    }
}

fn web_path() -> &'static str {
    if cfg!(target_os = "macos") {
        "Contents/Resources/web"
    } else {
        "web"
    }
}

#[cfg(target_os = "macos")]
fn extract(archive: &Path, destination: &Path) -> Result<PathBuf> {
    ensure!(
        background(Path::new("/usr/bin/ditto"), destination.parent().unwrap())
            .args(["-x", "-k"])
            .arg(archive)
            .arg(destination)
            .status()?
            .success(),
        "could not extract update"
    );
    let payload = destination.join(format!(
        "macos-{}-apple-darwin/LANLink.app",
        std::env::consts::ARCH
    ));
    ensure!(
        background(Path::new("/usr/bin/codesign"), destination)
            .args(["--verify", "--deep", "--strict"])
            .arg(&payload)
            .status()?
            .success(),
        "Mac update signature is invalid"
    );
    Ok(payload)
}

#[cfg(windows)]
fn extract(archive: &Path, destination: &Path) -> Result<PathBuf> {
    let system =
        PathBuf::from(std::env::var_os("SystemRoot").context("missing Windows system directory")?);
    let powershell = system.join("System32/WindowsPowerShell/v1.0/powershell.exe");
    ensure!(background(&powershell, archive.parent().unwrap())
        .args(["-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference = 'Stop'; Expand-Archive -LiteralPath $env:LANLINK_UPDATE_ZIP -DestinationPath $env:LANLINK_UPDATE_DIR"])
        .env("LANLINK_UPDATE_ZIP", archive)
        .env("LANLINK_UPDATE_DIR", destination)
        .status()?.success(), "could not extract update");
    ensure!(
        destination.join("Launch.ps1").is_file(),
        "Windows release launcher is missing"
    );
    Ok(destination.to_path_buf())
}

#[cfg(not(any(target_os = "macos", windows)))]
fn extract(_: &Path, _: &Path) -> Result<PathBuf> {
    anyhow::bail!("unsupported installation")
}
