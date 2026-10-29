//! Preparing, starting and supervising the game process.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use tokio_util::sync::CancellationToken;

use rustcraft_assets::java::{RUNTIME_INDEX_URL, runtime_platform};
use rustcraft_assets::{LaunchAuth, LaunchCommand, LaunchOptions, Progress, VersionJson, build_command};
use rustcraft_auth::AuthError;
use rustcraft_core::DataDir;
use rustcraft_core::fs::unix_time;
use rustcraft_mods::scan;
use rustcraft_platform::{discover_java, pick_java, probe_java};

use crate::config::{Engine, Instance, Loader, NATIVE_VERSION};
use crate::core::Core;

/// Lines that show the game has created its window and is running.
const READY_MARKERS: &[&str] = &["Setting user:", "LWJGL Version", "Backend library:", "RUSTCRAFT_READY"];

/// If no marker appears, the launcher still hides after this long.
pub const READY_FALLBACK: Duration = Duration::from_secs(40);

/// Offline (singleplayer-only) play is offered for accounts whose ownership
/// was verified online this recently.
const OFFLINE_GRACE_SECS: u64 = 30 * 24 * 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Account,
    Loader,
    Metadata,
    Download,
    Java,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Account => "Checking your account",
            Self::Loader => "Installing mod loader",
            Self::Metadata => "Reading version metadata",
            Self::Download => "Downloading game files",
            Self::Java => "Preparing Java",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LaunchError {
    pub message: String,
    /// The failure was a network problem and the account may play offline.
    pub offline_possible: bool,
}

impl<E: std::fmt::Display> From<E> for LaunchError {
    fn from(e: E) -> Self {
        Self { message: e.to_string(), offline_possible: false }
    }
}

fn fail(message: impl Into<String>) -> LaunchError {
    LaunchError { message: message.into(), offline_possible: false }
}

/// A command ready to start, with data for its standard input.
pub struct Prepared {
    pub command: LaunchCommand,
    pub stdin: Option<Vec<u8>>,
    /// Mods that will not load, one line each; the launch goes ahead.
    pub mod_problems: Vec<String>,
}

/// Everything up to (not including) starting the process: account, loader,
/// files, Java, command line.
pub async fn prepare(
    core: &Core,
    inst: &Instance,
    manifest: Option<Arc<rustcraft_assets::VersionManifest>>,
    offline: bool,
    progress: &Arc<Progress>,
    cancel: &CancellationToken,
    stage: &(dyn Fn(Stage) + Send + Sync),
) -> Result<Prepared, LaunchError> {
    if inst.engine == Engine::Native {
        return prepare_native(core, inst).await;
    }
    let command = prepare_java(core, inst, manifest, offline, progress, cancel, stage).await?;
    let (inst, dirs) = (inst.clone(), core.dirs.clone());
    let mod_problems = tokio::task::spawn_blocking(move || mod_problems(&inst, &dirs)).await?;
    Ok(Prepared { command, stdin: None, mod_problems })
}

/// "Title: first problem" for each enabled mod that will not load.
pub fn mod_problems(inst: &Instance, dirs: &DataDir) -> Vec<String> {
    let Ok(dir) = inst.dir(dirs) else { return Vec::new() };
    let mods = scan::scan(&dir.join("mods"));
    let checks = scan::check(&mods, &mod_target(inst));
    mods.iter().zip(checks).filter_map(|(m, c)| Some(format!("{}: {}", m.title(), c.problems.first()?))).collect()
}

pub fn mod_target(inst: &Instance) -> scan::Target<'_> {
    let fabric_loader = match &inst.loader {
        Loader::Fabric { version } => Some(version.as_str()),
        Loader::Vanilla => None,
    };
    scan::Target { game_version: &inst.game_version, fabric_loader }
}

/// The native engine needs no download: it uses the instance's resource
/// packs and, if a signed-in launch already downloaded it, the official
/// 1.8.9 client jar's textures; otherwise its own built-in art.
async fn prepare_native(core: &Core, inst: &Instance) -> Result<Prepared, LaunchError> {
    if inst.game_version != NATIVE_VERSION {
        return Err(fail(format!("The RustCraft engine only supports {NATIVE_VERSION}.")));
    }
    let exe_name = if cfg!(windows) { "rustcraft-client.exe" } else { "rustcraft-client" };
    let exe = std::env::current_exe()?.with_file_name(exe_name);
    if !exe.exists() {
        return Err(fail(format!("The RustCraft engine ({exe_name}) is missing from this installation.")));
    }
    let game_dir = inst.dir(&core.dirs)?;
    let mut packs: Vec<PathBuf> = std::fs::read_dir(game_dir.join("resourcepacks"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() || p.extension().is_some_and(|x| x == "zip"))
        .collect();
    packs.sort();
    let jar = core.dirs.version_dir(NATIVE_VERSION)?.join(format!("{NATIVE_VERSION}.jar"));
    if jar.exists() {
        packs.push(jar);
    }
    let server = inst.server.clone().filter(|s| !s.trim().is_empty());
    let mut accounts = core.accounts.lock().await;
    let account = accounts.active().cloned();
    // Online-mode servers need a session; offline-mode ones accept any name.
    let mut token = None;
    if server.is_some()
        && let (Some(record), Some(auth)) = (&account, core.auth.as_ref())
    {
        match accounts.session(auth, &record.id).await {
            Ok(session) => {
                if let Some(reason) = session.multiplayer.blocked_reason() {
                    return Err(fail(reason));
                }
                token = Some(session.access_token.expose().to_string());
            }
            Err(e) => tracing::warn!("no Minecraft session ({e}); only offline-mode servers will let the player in"),
        }
    }
    drop(accounts);
    let spec = serde_json::json!({
        "game_dir": game_dir,
        "resource_packs": packs,
        "world": "World",
        "server": server,
        "player_name": account.as_ref().map_or("Player", |a| a.name.as_str()),
        "player_uuid": account.as_ref().map_or("", |a| a.id.as_str()),
        "access_token": token,
    });
    Ok(Prepared {
        // The token travels in the spec on stdin; it is also redacted from
        // the game's output in the log.
        command: LaunchCommand::new(exe, vec!["--spec-stdin".into()], game_dir, token.unwrap_or_default()),
        stdin: Some(spec.to_string().into_bytes()),
        mod_problems: Vec::new(),
    })
}

async fn prepare_java(
    core: &Core,
    inst: &Instance,
    manifest: Option<Arc<rustcraft_assets::VersionManifest>>,
    offline: bool,
    progress: &Arc<Progress>,
    cancel: &CancellationToken,
    stage: &(dyn Fn(Stage) + Send + Sync),
) -> Result<LaunchCommand, LaunchError> {
    stage(Stage::Account);
    let auth = launch_auth(core, inst, offline).await?;

    let version_id = match &inst.loader {
        Loader::Vanilla => inst.game_version.clone(),
        Loader::Fabric { version } => {
            let id = format!("fabric-loader-{version}-{}", inst.game_version);
            if !core.dirs.version_dir(&id)?.join(format!("{id}.json")).exists() {
                stage(Stage::Loader);
                rustcraft_mods::loader::install_fabric(
                    &core.http,
                    rustcraft_mods::loader::FABRIC_META,
                    &core.dirs,
                    &inst.game_version,
                    version,
                )
                .await?;
            }
            id
        }
    };

    stage(Stage::Metadata);
    let version = core.installer.resolve(&version_id, manifest.as_deref()).await?;

    stage(Stage::Download);
    let installed = core.installer.install(&version, progress, cancel).await?;

    stage(Stage::Java);
    let java = choose_java(core, inst, &version, progress, cancel).await?;

    let opts = LaunchOptions {
        java,
        game_dir: inst.dir(&core.dirs)?,
        assets_dir: core.dirs.assets(),
        libraries_dir: core.dirs.libraries(),
        auth,
        min_memory_mb: inst.min_memory_mb,
        max_memory_mb: inst.max_memory_mb,
        // ponytail: whitespace split; quoted arguments with spaces are not supported.
        extra_jvm_args: inst.jvm_args.split_whitespace().map(Into::into).collect(),
        resolution: inst.resolution,
        server: inst.server.clone().filter(|s| !s.trim().is_empty()),
    };
    Ok(build_command(&installed, &opts, core.installer.platform)?)
}

async fn launch_auth(core: &Core, inst: &Instance, offline: bool) -> Result<LaunchAuth, LaunchError> {
    let mut accounts = core.accounts.lock().await;
    let record = accounts
        .active()
        .cloned()
        .ok_or_else(|| fail("Sign in with a Microsoft account that owns Minecraft: Java Edition to play."))?;
    let recently_verified = unix_time().saturating_sub(record.verified_at) < OFFLINE_GRACE_SECS;
    if offline {
        if !recently_verified {
            return Err(fail("Offline play needs an account verified online in the last 30 days."));
        }
        // The game accepts any token for singleplayer; online features stay unavailable.
        return Ok(LaunchAuth {
            player_name: record.name,
            uuid: record.id,
            access_token: "0".into(),
            user_type: "msa".into(),
            xuid: "0".into(),
            client_id: String::new(),
        });
    }
    let auth = core.auth.as_ref().ok_or_else(|| fail(AuthError::NotConfigured.to_string()))?;
    let session = accounts.session(auth, &record.id).await.map_err(|e| LaunchError {
        offline_possible: matches!(e, AuthError::Network { .. }) && recently_verified,
        message: e.to_string(),
    })?;
    if inst.server.as_deref().is_some_and(|s| !s.trim().is_empty())
        && let Some(reason) = session.multiplayer.blocked_reason()
    {
        return Err(fail(reason));
    }
    Ok(LaunchAuth {
        player_name: session.profile.name,
        uuid: session.profile.id,
        access_token: session.access_token.expose().into(),
        user_type: "msa".into(),
        xuid: "0".into(),
        client_id: auth.config().client_id.clone(),
    })
}

async fn choose_java(
    core: &Core,
    inst: &Instance,
    version: &VersionJson,
    progress: &Arc<Progress>,
    cancel: &CancellationToken,
) -> Result<PathBuf, LaunchError> {
    let need = version.java_major();
    let override_path = inst.java_path.clone().or_else(|| core.settings.lock().unwrap().java_path.clone());
    if let Some(path) = override_path {
        let probe_path = path.clone();
        let java = tokio::task::spawn_blocking(move || probe_java(&probe_path))
            .await?
            .ok_or_else(|| fail(format!("The Java at {} could not be run.", path.display())))?;
        if java.major < need {
            return Err(fail(format!(
                "{} needs Java {need}, but the selected Java is {}. Choose another Java in Settings.",
                version.id, java.major
            )));
        }
        return Ok(java.path);
    }

    let component = version.java_version.as_ref().map(|j| j.component.clone()).unwrap_or_else(|| "jre-legacy".into());
    if runtime_platform(core.installer.platform).is_some() {
        match core.installer.install_java(RUNTIME_INDEX_URL, &component, progress, cancel).await {
            Ok(path) => return Ok(path),
            Err(rustcraft_assets::AssetsError::Download(rustcraft_assets::DownloadError::Cancelled)) => {
                return Err(fail("Cancelled."));
            }
            Err(e) => {
                tracing::warn!("Mojang Java runtime {component} unavailable ({e}); looking for an installed Java")
            }
        }
    }
    let installs = tokio::task::spawn_blocking(discover_java).await?;
    pick_java(&installs, need).map(|j| j.path.clone()).ok_or_else(|| {
        fail(format!(
            "{} needs Java {need}. Install it, or reconnect so RustCraft can download Mojang's Java runtime.",
            version.id
        ))
    })
}

pub struct RunningGame {
    pub child: Child,
    pub instance: String,
    pub game_dir: PathBuf,
    pub log_path: PathBuf,
    pub started: SystemTime,
    pub started_at: Instant,
    pub ready: Arc<AtomicBool>,
    /// What the native client said when it gave up, if anything.
    reason: Arc<Mutex<Option<String>>>,
    pumps_done: std::sync::mpsc::Receiver<()>,
}

#[derive(Debug, Clone)]
pub struct GameOutcome {
    pub instance: String,
    pub exit_code: Option<i32>,
    pub success: bool,
    /// Exited before the game window came up.
    pub early: bool,
    pub log_path: PathBuf,
    pub crash_report: Option<PathBuf>,
    pub played: Duration,
    pub reason: Option<String>,
}

impl GameOutcome {
    pub fn summary(&self) -> String {
        let summary = self.status();
        match &self.reason {
            Some(reason) if !self.success => format!("{summary} {reason}"),
            _ => summary,
        }
    }

    fn status(&self) -> String {
        match (self.success, self.exit_code) {
            (true, _) => format!("{} closed normally after {}.", self.instance, format_duration(self.played)),
            (false, Some(code)) if self.early => {
                format!("{} failed to start (exit code {code}).", self.instance)
            }
            (false, Some(code)) => format!("{} crashed (exit code {code}).", self.instance),
            (false, None) => format!("{} was terminated.", self.instance),
        }
    }
}

/// "3 days ago" style age of a Unix timestamp.
pub fn format_ago(unix: u64) -> String {
    let secs = unix_time().saturating_sub(unix);
    let (n, unit) = match secs {
        0..60 => return "just now".into(),
        60..3600 => (secs / 60, "minute"),
        3600..86_400 => (secs / 3600, "hour"),
        86_400..2_592_000 => (secs / 86_400, "day"),
        2_592_000..31_536_000 => (secs / 2_592_000, "month"),
        _ => (secs / 31_536_000, "year"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

pub fn format_duration(d: Duration) -> String {
    let mins = d.as_secs() / 60;
    if mins >= 60 { format!("{}h {}m", mins / 60, mins % 60) } else { format!("{}m {}s", mins, d.as_secs() % 60) }
}

/// Starts the game with its output captured to a log file in `logs_dir`.
pub fn spawn(
    cmd: &LaunchCommand,
    stdin: Option<Vec<u8>>,
    instance: &str,
    logs_dir: &Path,
    on_ready: impl Fn() + Send + Sync + Clone + 'static,
) -> io::Result<RunningGame> {
    std::fs::create_dir_all(logs_dir)?;
    let safe_name: String = instance.chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' }).collect();
    let log_path = logs_dir.join(format!("{safe_name}-{}.log", unix_time()));
    let mut log = io::LineWriter::new(std::fs::File::create(&log_path)?);
    writeln!(log, "[RustCraft] {} {}", cmd.program.display(), cmd.redacted_args().join(" "))?;
    let log = Arc::new(Mutex::new(log));

    let mut command = cmd.to_command();
    command
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn()?;
    if let (Some(data), Some(mut pipe)) = (stdin, child.stdin.take()) {
        // Launch data (which may hold a session token) goes over a pipe,
        // never the command line.
        std::thread::spawn(move || {
            let _ = pipe.write_all(&data);
        });
    }
    let ready = Arc::new(AtomicBool::new(false));
    let reason = Arc::new(Mutex::new(None));
    let (done, pumps_done) = std::sync::mpsc::channel();
    let streams: [Option<Box<dyn Read + Send>>; 2] = [
        child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
        child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
    ];
    for stream in streams.into_iter().flatten() {
        let (cmd, log, ready, reason, on_ready, done) =
            (cmd.clone(), log.clone(), ready.clone(), reason.clone(), on_ready.clone(), done.clone());
        std::thread::Builder::new().name("game-log".into()).spawn(move || {
            pump(stream, &cmd, &log, &ready, &reason, &on_ready);
            let _ = done.send(());
        })?;
    }
    Ok(RunningGame {
        child,
        instance: instance.into(),
        game_dir: cmd.cwd.clone(),
        log_path,
        started: SystemTime::now(),
        started_at: Instant::now(),
        ready,
        reason,
        pumps_done,
    })
}

/// How the native client prefixes the reason it stopped (`rustcraft-client`'s main).
const STOPPED_PREFIX: &str = "RustCraft stopped: ";

fn pump(
    stream: impl Read,
    cmd: &LaunchCommand,
    log: &Mutex<impl Write>,
    ready: &AtomicBool,
    reason: &Mutex<Option<String>>,
    on_ready: &dyn Fn(),
) {
    let mut reader = BufReader::new(stream);
    let mut parser = Log4jParser::default();
    let mut raw = Vec::new();
    loop {
        raw.clear();
        match reader.read_until(b'\n', &mut raw) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let line = String::from_utf8_lossy(&raw);
        let Some(text) = parser.feed(line.trim_end_matches(['\r', '\n'])) else {
            continue;
        };
        // Some versions print the session token; keep it out of the log.
        let text = cmd.redact(&text);
        if !ready.load(Ordering::Relaxed) && READY_MARKERS.iter().any(|m| text.contains(m)) {
            ready.store(true, Ordering::Relaxed);
            on_ready();
        }
        if let Some(r) = text.strip_prefix(STOPPED_PREFIX) {
            *reason.lock().unwrap() = Some(r.to_string());
        }
        let _ = writeln!(log.lock().unwrap(), "{text}");
    }
}

impl RunningGame {
    pub fn outcome(&self, status: io::Result<ExitStatus>) -> GameOutcome {
        // Let the log threads take in the last lines the process wrote. A
        // child the game left running may keep its pipes open; don't wait on it.
        for _ in 0..2 {
            let _ = self.pumps_done.recv_timeout(Duration::from_secs(1));
        }
        let exit_code = status.as_ref().ok().and_then(|s| s.code());
        let success = status.as_ref().is_ok_and(|s| s.success());
        GameOutcome {
            instance: self.instance.clone(),
            exit_code,
            success,
            early: !self.ready.load(Ordering::Relaxed),
            log_path: self.log_path.clone(),
            crash_report: if success { None } else { newest_crash_report(&self.game_dir, self.started) },
            played: self.started_at.elapsed(),
            reason: self.reason.lock().unwrap().clone(),
        }
    }
}

/// The newest crash report or JVM fatal-error log written since `since`.
fn newest_crash_report(game_dir: &Path, since: SystemTime) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    for dir in [game_dir.join("crash-reports"), game_dir.to_path_buf()] {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_report = (name.starts_with("crash-") && name.ends_with(".txt"))
                || (name.starts_with("hs_err_pid") && name.ends_with(".log"));
            let modified = entry.metadata().and_then(|m| m.modified()).ok();
            if let Some(modified) = modified.filter(|m| is_report && *m >= since) {
                candidates.push((modified, entry.path()));
            }
        }
    }
    candidates.into_iter().max_by_key(|(m, _)| *m).map(|(_, p)| p)
}

/// Turns Log4j2 XML events (the official client's stdout format when its
/// logging config is used) into readable lines; other lines pass through.
#[derive(Default)]
struct Log4jParser {
    event: Option<String>,
}

impl Log4jParser {
    fn feed(&mut self, line: &str) -> Option<String> {
        let trimmed = line.trim();
        if let Some(buf) = &mut self.event {
            buf.push_str(line);
            buf.push('\n');
            if trimmed.ends_with("</log4j:Event>") {
                return self.event.take().map(|xml| format_event(&xml));
            }
            return None;
        }
        if trimmed.starts_with("<log4j:Event") {
            if trimmed.ends_with("</log4j:Event>") {
                return Some(format_event(trimmed));
            }
            self.event = Some(format!("{line}\n"));
            return None;
        }
        Some(line.to_string())
    }
}

fn format_event(xml: &str) -> String {
    let attr = |name: &str| {
        xml.split(&format!(" {name}=\"")).nth(1).and_then(|s| s.split('"').next()).unwrap_or("").to_string()
    };
    let mut cdata = xml.split("<![CDATA[").skip(1).filter_map(|s| s.split("]]>").next());
    let message = cdata.next().unwrap_or("").trim_end();
    let mut line = format!("[{}/{}]: {message}", attr("thread"), attr("level"));
    if let Some(throwable) = cdata.next() {
        line.push('\n');
        line.push_str(throwable.trim_end());
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log4j_events_become_lines() {
        let mut p = Log4jParser::default();
        assert_eq!(p.feed("plain output").as_deref(), Some("plain output"));
        assert_eq!(p.feed(r#"<log4j:Event logger="ave" timestamp="1" level="INFO" thread="Client thread">"#), None);
        assert_eq!(p.feed("  <log4j:Message><![CDATA[Setting user: Notch]]></log4j:Message>"), None);
        assert_eq!(p.feed("</log4j:Event>").as_deref(), Some("[Client thread/INFO]: Setting user: Notch"));
        let one = r#"<log4j:Event logger="x" timestamp="1" level="ERROR" thread="Render thread"><log4j:Message><![CDATA[boom]]></log4j:Message><log4j:Throwable><![CDATA[java.lang.RuntimeException: boom
	at a.b(c.java:1)]]></log4j:Throwable></log4j:Event>"#;
        // A multi-line throwable arrives across lines.
        let mut lines = one.lines();
        assert_eq!(p.feed(lines.next().unwrap()), None);
        let out = p.feed(lines.next().unwrap()).unwrap();
        assert!(out.starts_with("[Render thread/ERROR]: boom\njava.lang.RuntimeException: boom"), "{out}");
    }

    #[cfg(unix)]
    #[test]
    fn spawn_captures_output_and_detects_ready() {
        let tmp = tempfile::tempdir().unwrap();
        let script = "echo 'starting'; echo 'Setting user: Notch'; echo '(Session ID is token:T0K3N:1)'; \
                      echo 'oops' 1>&2; echo 'RustCraft stopped: out of luck' 1>&2; exit 3";
        let cmd =
            LaunchCommand::new("/bin/sh".into(), vec!["-c".into(), script.into()], tmp.path().into(), "T0K3N".into());
        let fired = Arc::new(AtomicBool::new(false));
        let f = fired.clone();
        let mut game = spawn(&cmd, None, "Test 1", tmp.path(), move || f.store(true, Ordering::Relaxed)).unwrap();
        let status = game.child.wait();
        let outcome = game.outcome(status);
        assert!(fired.load(Ordering::Relaxed));
        assert_eq!(outcome.exit_code, Some(3));
        assert!(!outcome.success && !outcome.early);
        let log = std::fs::read_to_string(&outcome.log_path).unwrap();
        assert!(log.contains("Setting user: Notch") && log.contains("oops"), "{log}");
        assert!(log.contains("token:<redacted>:1") && !log.contains("T0K3N"), "{log}");
        assert_eq!(outcome.summary(), "Test 1 crashed (exit code 3). out of luck");
    }

    #[test]
    fn finds_crash_report_written_during_session() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("crash-reports")).unwrap();
        let since = SystemTime::now() - Duration::from_secs(5);
        std::fs::write(tmp.path().join("crash-reports/crash-2025-01-01_00.00.00-client.txt"), "x").unwrap();
        std::fs::write(tmp.path().join("notes.txt"), "x").unwrap();
        let found = newest_crash_report(tmp.path(), since).unwrap();
        assert!(found.ends_with("crash-2025-01-01_00.00.00-client.txt"));
        assert!(newest_crash_report(tmp.path(), SystemTime::now() + Duration::from_secs(60)).is_none());
    }
}
