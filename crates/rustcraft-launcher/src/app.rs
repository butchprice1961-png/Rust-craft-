//! The launcher window: state, background tasks and their results.
//! Drawing lives in `views.rs`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use eframe::egui;
use tokio_util::sync::CancellationToken;

use rustcraft_assets::{Progress, VersionManifest};
use rustcraft_auth::AccountRecord;
use rustcraft_core::fs::unix_time;
use rustcraft_mods::loader::FabricLoader;
use rustcraft_platform::JavaInstall;

use crate::config::{Graphics, Instance, load_instances};
use crate::core::Core;
use crate::game::{self, GameOutcome, LaunchError, READY_FALLBACK, Stage};
use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Play,
    Instances,
    Accounts,
    Settings,
    Diagnostics,
}

pub enum Event {
    Manifest(Result<Arc<VersionManifest>, String>),
    Accounts(AccountsView),
    SignInUrl(String),
    SignInDone(SignInResult),
    Stage(Stage),
    Spawned,
    LaunchFailed(LaunchError),
    LaunchCancelled,
    Java(Vec<JavaInstall>),
    Loaders(String, Result<Vec<FabricLoader>, String>),
    Notice(Level, String),
}

pub enum SignInResult {
    /// Signed in; carries a warning if the credential could not be saved.
    Ok(Option<String>),
    Cancelled,
    Failed(String),
}

#[derive(Debug, Clone, Default)]
pub struct AccountsView {
    pub records: Vec<AccountRecord>,
    pub active: Option<String>,
}

impl AccountsView {
    pub fn active(&self) -> Option<&AccountRecord> {
        let id = self.active.as_deref()?;
        self.records.iter().find(|r| r.id == id)
    }
}

pub enum PlayState {
    Idle,
    Preparing { stage: Stage, progress: Arc<Progress>, cancel: CancellationToken },
    Starting,
    Failed(LaunchError),
}

pub enum SignIn {
    Idle,
    Waiting { url: Option<String>, cancel: CancellationToken },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

pub struct Notice {
    pub level: Level,
    pub text: String,
    pub at: Instant,
}

pub enum Loaders {
    Loading,
    Loaded(Vec<FabricLoader>),
    Failed(String),
}

pub struct NewInstance {
    pub name: String,
    pub version: Option<String>,
    /// Use RustCraft's own engine (1.8.9 only).
    pub native: bool,
    pub filter: String,
    pub snapshots: bool,
    pub fabric: bool,
    pub loaders: Option<(String, Loaders)>,
    pub loader_choice: Option<String>,
    pub view_distance: u32,
    pub graphics: Graphics,
    pub error: Option<String>,
}

/// `--smoke-test <png>`: render, screenshot, exit. Used to check packaged builds.
pub struct SmokeTest {
    pub output: PathBuf,
    /// Start the selected instance (offline) on the first frame.
    pub launch: bool,
    frames: u32,
    requested: bool,
}

pub struct LauncherApp {
    pub core: Arc<Core>,
    pub tx: Sender<Event>,
    rx: Receiver<Event>,
    pub ctx: egui::Context,
    pub tab: Tab,
    pub accounts: AccountsView,
    pub manifest: Option<Arc<VersionManifest>>,
    pub manifest_error: Option<String>,
    pub instances: Vec<Instance>,
    pub play: PlayState,
    launch_task: Option<tokio::task::JoinHandle<()>>,
    pub sign_in: SignIn,
    pub notices: Vec<Notice>,
    pub new_instance: Option<NewInstance>,
    pub edit: Option<Instance>,
    pub confirm_delete: Option<String>,
    pub java: Option<Vec<JavaInstall>>,
    pub java_path_text: String,
    pub last_outcome: Option<GameOutcome>,
    pub smoke: Option<SmokeTest>,
    closing: bool,
}

impl LauncherApp {
    pub fn new(cc: &eframe::CreationContext<'_>, core: Arc<Core>, smoke: Option<crate::Smoke>) -> Self {
        let settings = core.settings.lock().unwrap().clone();
        theme::install(&cc.egui_ctx, settings.reduced_motion);
        let (tx, rx) = channel();
        let accounts = snapshot(&core.accounts.blocking_lock());
        let last_outcome = core.last_outcome.lock().unwrap().clone();
        let settings_notice = core.settings_notice.lock().unwrap().take();
        let mut app = Self {
            tx,
            rx,
            ctx: cc.egui_ctx.clone(),
            tab: Tab::Play,
            accounts,
            manifest: None,
            manifest_error: None,
            instances: load_instances(&core.dirs),
            play: PlayState::Idle,
            launch_task: None,
            sign_in: SignIn::Idle,
            notices: Vec::new(),
            new_instance: None,
            edit: None,
            confirm_delete: None,
            java: None,
            java_path_text: settings.java_path.map(|p| p.display().to_string()).unwrap_or_default(),
            last_outcome,
            closing: false,
            smoke: smoke.as_ref().map(|s| SmokeTest {
                output: s.output.clone(),
                launch: s.launch,
                frames: 0,
                requested: false,
            }),
            core,
        };
        if let Some(notice) = settings_notice {
            app.notify(Level::Warn, notice);
        }
        app.tab = match smoke.and_then(|s| s.tab).as_deref() {
            Some("instances") => Tab::Instances,
            Some("accounts") => Tab::Accounts,
            Some("settings") => Tab::Settings,
            Some("help") => Tab::Diagnostics,
            Some("new") => {
                app.open_new_instance();
                Tab::Instances
            }
            _ => Tab::Play,
        };
        // Show the cached version list at once; the fresh one replaces it.
        app.manifest = app.core.installer.cached_manifest().map(Arc::new);
        app.refresh_manifest();
        app.discover_java();
        app
    }

    pub fn notify(&mut self, level: Level, text: impl Into<String>) {
        let text = text.into();
        match level {
            Level::Error => tracing::error!("{text}"),
            Level::Warn => tracing::warn!("{text}"),
            Level::Info => tracing::info!("{text}"),
        }
        self.notices.push(Notice { level, text, at: Instant::now() });
    }

    /// Runs `work` on the I/O runtime and delivers its event to the UI.
    fn spawn<F>(&self, work: F) -> tokio::task::JoinHandle<()>
    where
        F: Future<Output = Event> + Send + 'static,
    {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.core.rt.spawn(async move {
            let _ = tx.send(work.await);
            ctx.request_repaint();
        })
    }

    pub fn refresh_manifest(&mut self) {
        let core = self.core.clone();
        self.spawn(
            async move { Event::Manifest(core.installer.manifest().await.map(Arc::new).map_err(|e| e.to_string())) },
        );
    }

    pub fn discover_java(&mut self) {
        self.spawn(async move {
            Event::Java(tokio::task::spawn_blocking(rustcraft_platform::discover_java).await.unwrap_or_default())
        });
    }

    pub fn selected_instance(&self) -> Option<&Instance> {
        let selected = self.core.settings.lock().unwrap().selected_instance.clone();
        self.instances.iter().find(|i| Some(&i.name) == selected.as_ref()).or(self.instances.first())
    }

    pub fn select_instance(&mut self, name: &str) {
        self.core.settings.lock().unwrap().selected_instance = Some(name.into());
        self.core.save_settings();
    }

    pub fn reload_instances(&mut self) {
        self.instances = load_instances(&self.core.dirs);
    }

    pub fn is_installed(&self, version: &str) -> bool {
        self.core.dirs.version_dir(version).is_ok_and(|d| d.join(format!("{version}.jar")).exists())
    }

    pub fn busy(&self) -> bool {
        matches!(self.play, PlayState::Preparing { .. } | PlayState::Starting)
    }

    pub fn start_launch(&mut self, offline: bool) {
        let Some(inst) = self.selected_instance().cloned() else {
            return;
        };
        let progress = Arc::new(Progress::default());
        let cancel = CancellationToken::new();
        self.play = PlayState::Preparing { stage: Stage::Account, progress: progress.clone(), cancel: cancel.clone() };
        self.last_outcome = None;
        *self.core.last_outcome.lock().unwrap() = None;
        let (core, tx, ctx) = (self.core.clone(), self.tx.clone(), self.ctx.clone());
        let manifest = self.manifest.clone();
        self.launch_task = Some(self.spawn(async move {
            let stage = {
                let (tx, ctx) = (tx.clone(), ctx.clone());
                move |s| {
                    let _ = tx.send(Event::Stage(s));
                    ctx.request_repaint();
                }
            };
            let prepared = game::prepare(&core, &inst, manifest, offline, &progress, &cancel, &stage).await;
            if cancel.is_cancelled() {
                return Event::LaunchCancelled;
            }
            let prepared = match prepared {
                Ok(p) => p,
                Err(e) => return Event::LaunchFailed(e),
            };
            tracing::info!("launching {}: {:?}", inst.name, prepared.command.redacted_args());
            let on_ready = move || ctx.request_repaint();
            match game::spawn(&prepared.command, prepared.stdin, &inst.name, &core.dirs.logs(), on_ready) {
                Ok(running) => {
                    *core.game.lock().unwrap() = Some(running);
                    let mut played = inst.clone();
                    played.last_played = Some(unix_time());
                    let _ = played.save(&core.dirs);
                    Event::Spawned
                }
                Err(e) => Event::LaunchFailed(LaunchError {
                    message: format!("Could not start Java: {e}"),
                    offline_possible: false,
                }),
            }
        }));
    }

    pub fn cancel_launch(&mut self) {
        if let PlayState::Preparing { cancel, .. } = &self.play {
            cancel.cancel();
        }
    }

    pub fn start_sign_in(&mut self) {
        let Some(auth) = self.core.auth.clone() else {
            self.notify(Level::Error, rustcraft_auth::AuthError::NotConfigured.to_string());
            return;
        };
        let cancel = CancellationToken::new();
        self.sign_in = SignIn::Waiting { url: None, cancel: cancel.clone() };
        let (core, tx, ctx) = (self.core.clone(), self.tx.clone(), self.ctx.clone());
        let list_tx = self.tx.clone();
        self.spawn(async move {
            let on_url = move |url: &str| {
                if let Err(e) = rustcraft_auth::open_in_browser(url) {
                    tracing::warn!("could not open browser: {e}");
                }
                let _ = tx.send(Event::SignInUrl(url.into()));
                ctx.request_repaint();
            };
            match auth.sign_in(on_url, &cancel).await {
                Ok(signed_in) => {
                    let mut accounts = core.accounts.lock().await;
                    let result = accounts.add(signed_in);
                    let _ = list_tx.send(Event::Accounts(snapshot(&accounts)));
                    match result {
                        Ok(warning) => Event::SignInDone(SignInResult::Ok(warning)),
                        Err(e) => Event::SignInDone(SignInResult::Failed(format!("Could not save the account: {e}"))),
                    }
                }
                Err(rustcraft_auth::AuthError::Cancelled) => Event::SignInDone(SignInResult::Cancelled),
                Err(e) => Event::SignInDone(SignInResult::Failed(e.to_string())),
            }
        });
    }

    pub fn update_accounts(&mut self, change: AccountChange) {
        let core = self.core.clone();
        self.spawn(async move {
            let mut accounts = core.accounts.lock().await;
            let result = match &change {
                AccountChange::Activate(id) => accounts.set_active(id),
                AccountChange::Remove(id) => accounts.remove(id),
            };
            if let Err(e) = result {
                return Event::Notice(Level::Error, format!("Could not update accounts: {e}"));
            }
            Event::Accounts(snapshot(&accounts))
        });
    }

    pub fn load_fabric_loaders(&mut self, game_version: String) {
        let http = self.core.http.clone();
        self.spawn(async move {
            let result =
                rustcraft_mods::loader::fabric_loaders(&http, rustcraft_mods::loader::FABRIC_META, &game_version)
                    .await
                    .map_err(|e| e.to_string());
            Event::Loaders(game_version, result)
        });
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Manifest(Ok(m)) => {
                self.manifest = Some(m);
                self.manifest_error = None;
            }
            Event::Manifest(Err(e)) if self.manifest.is_none() => self.manifest_error = Some(e),
            Event::Manifest(Err(e)) => {
                tracing::warn!("version list refresh failed, keeping cached copy: {e}")
            }
            Event::Accounts(view) => self.accounts = view,
            Event::SignInUrl(url) => {
                if let SignIn::Waiting { url: slot, .. } = &mut self.sign_in {
                    *slot = Some(url);
                }
            }
            Event::SignInDone(result) => {
                self.sign_in = SignIn::Idle;
                match result {
                    SignInResult::Ok(warning) => {
                        let name = self.accounts.active().map(|a| a.name.clone()).unwrap_or_default();
                        self.notify(Level::Info, format!("Signed in as {name}."));
                        if let Some(w) = warning {
                            self.notify(Level::Warn, w);
                        }
                    }
                    SignInResult::Cancelled => {}
                    SignInResult::Failed(e) => self.notify(Level::Error, e),
                }
            }
            Event::Stage(stage) => {
                if let PlayState::Preparing { stage: s, .. } = &mut self.play {
                    *s = stage;
                }
            }
            Event::Spawned => self.play = PlayState::Starting,
            Event::LaunchFailed(e) => {
                tracing::error!("launch failed: {}", e.message);
                self.play = PlayState::Failed(e);
            }
            Event::LaunchCancelled => self.play = PlayState::Idle,
            Event::Java(found) => self.java = Some(found),
            Event::Loaders(game_version, result) => {
                if let Some(draft) = &mut self.new_instance {
                    let loaders = match result {
                        Ok(list) => {
                            draft.loader_choice =
                                list.iter().find(|l| l.stable).or(list.first()).map(|l| l.version.clone());
                            Loaders::Loaded(list)
                        }
                        Err(e) => Loaders::Failed(e),
                    };
                    draft.loaders = Some((game_version, loaders));
                }
            }
            Event::Notice(level, text) => self.notify(level, text),
        }
    }

    /// Watches the starting game: hides the launcher once gameplay begins,
    /// or reports an early exit.
    fn check_game(&mut self, ctx: &egui::Context) {
        let mut guard = self.core.game.lock().unwrap();
        let Some(running) = guard.as_mut() else {
            return;
        };
        match running.child.try_wait() {
            Ok(Some(status)) => {
                let outcome = running.outcome(Ok(status));
                *guard = None;
                drop(guard);
                self.play = PlayState::Idle;
                *self.core.last_outcome.lock().unwrap() = Some(outcome.clone());
                self.last_outcome = Some(outcome);
            }
            Ok(None) => {
                if running.ready.load(Ordering::Relaxed) || running.started_at.elapsed() > READY_FALLBACK {
                    // The supervisor loop in main keeps watching the game and
                    // reopens this window when it exits.
                    if !std::mem::replace(&mut self.closing, true) {
                        tracing::info!("game is running; closing launcher window");
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                } else {
                    ctx.request_repaint_after(Duration::from_millis(250));
                }
            }
            Err(e) => tracing::warn!("could not poll game process: {e}"),
        }
    }

    fn smoke_test(&mut self, ctx: &egui::Context) {
        let Some(smoke) = &mut self.smoke else { return };
        if std::mem::take(&mut smoke.launch) {
            self.start_launch(true);
            return;
        }
        if self.busy() {
            // Waiting for the game; the window closes once it is running.
            return;
        }
        let Some(smoke) = &mut self.smoke else { return };
        smoke.frames += 1;
        if smoke.frames > 20 && !smoke.requested {
            smoke.requested = true;
            let (output, ctx2) = (smoke.output.clone(), ctx.clone());
            ctx.request_screenshot(move |image| {
                let [w, h] = image.size;
                let rgba: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
                match image::save_buffer(&output, &rgba, w as u32, h as u32, image::ExtendedColorType::Rgba8) {
                    Ok(()) => println!("smoke test: wrote {}", output.display()),
                    Err(e) => eprintln!("smoke test: could not save screenshot: {e}"),
                }
                ctx2.send_viewport_cmd(egui::ViewportCommand::Close);
                ctx2.request_repaint();
            });
        }
        ctx.request_repaint();
    }
}

pub enum AccountChange {
    Activate(String),
    Remove(String),
}

fn snapshot(accounts: &rustcraft_auth::Accounts) -> AccountsView {
    AccountsView { records: accounts.records().to_vec(), active: accounts.active().map(|r| r.id.clone()) }
}

impl eframe::App for LauncherApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        while let Ok(event) = self.rx.try_recv() {
            self.handle(event);
        }
        self.check_game(&ctx);
        self.draw(ui);
        self.smoke_test(&ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Closing during preparation cancels it; the supervisor then only
        // waits if the game had already started.
        self.cancel_launch();
        if let Some(task) = self.launch_task.take() {
            let _ = self.core.rt.block_on(task);
        }
        self.core.save_settings();
    }
}
