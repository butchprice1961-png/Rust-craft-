//! Drawing for each launcher screen.

use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Duration;

use eframe::egui::{
    self, Align, Align2, Color32, ComboBox, CornerRadius, FontId, Frame, Id, Layout, Margin, Modal, ProgressBar, Rect,
    Response, RichText, ScrollArea, Sense, Stroke, StrokeKind, TextEdit, Ui, Vec2, WidgetInfo,
};

use rustcraft_assets::VersionKind;

use crate::app::{AccountChange, LauncherApp, Level, Loaders, NewInstance, PlayState, SignIn, Tab};
use crate::config::{Engine, Graphics, Instance, Loader, NATIVE_VERSION, delete_instance, validate_name};
use crate::game::format_ago;
use crate::theme::{self, ACCENT, BORDER, CARD, CARD_RAISED, DANGER, MUTED, SUCCESS, TEXT, WARNING, muted};

const RELEASE_NOTES: &str = include_str!("../../../CHANGELOG.md");

impl LauncherApp {
    pub fn draw(&mut self, ui: &mut Ui) {
        egui::Panel::left("nav")
            .resizable(false)
            .exact_size(228.0)
            .show_separator_line(false)
            .frame(Frame::new().fill(theme::SIDEBAR).inner_margin(Margin::symmetric(14, 18)))
            .show(ui, |ui| self.sidebar(ui));
        egui::CentralPanel::no_frame()
            .frame(Frame::new().fill(theme::BG).inner_margin(Margin::symmetric(30, 26)))
            .show(ui, |ui| {
                ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    // Keep content clear of the floating scroll bar.
                    Frame::new().inner_margin(Margin { right: 14, ..Margin::ZERO }).show(ui, |ui| {
                        ui.set_max_width(ui.available_width().min(980.0));
                        match self.tab {
                            Tab::Play => self.view_play(ui),
                            Tab::Instances => self.view_instances(ui),
                            Tab::Accounts => self.view_accounts(ui),
                            Tab::Settings => self.view_settings(ui),
                            Tab::Diagnostics => self.view_diagnostics(ui),
                        }
                    });
                });
            });
        let ctx = ui.ctx().clone();
        self.new_instance_modal(&ctx);
        self.edit_modal(&ctx);
        self.delete_modal(&ctx);
        self.toasts(&ctx);
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            logo(ui, 36.0);
            ui.vertical(|ui| {
                ui.add_space(1.0);
                ui.label(RichText::new("RustCraft").family(theme::semibold()).size(19.0).color(TEXT));
                ui.label(muted("Launcher").size(12.0));
            });
        });
        ui.add_space(26.0);
        for (tab, icon, label) in [
            (Tab::Play, Icon::Play, "Play"),
            (Tab::Instances, Icon::Grid, "Instances"),
            (Tab::Accounts, Icon::Person, "Accounts"),
            (Tab::Settings, Icon::Sliders, "Settings"),
            (Tab::Diagnostics, Icon::Help, "Help & diagnostics"),
        ] {
            if nav_item(ui, self.tab == tab, icon, label).clicked() {
                self.tab = tab;
            }
            ui.add_space(2.0);
        }
        ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
            ui.label(muted(format!("v{}", env!("CARGO_PKG_VERSION"))).size(11.0));
            ui.add_space(6.0);
            let active = self.accounts.active().cloned();
            Frame::new().fill(CARD).corner_radius(CornerRadius::same(10)).inner_margin(Margin::same(10)).show(
                ui,
                |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| match &active {
                        Some(a) => {
                            avatar(ui, &a.name, 30.0);
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&a.name).family(theme::semibold()).color(TEXT));
                                ui.label(muted("Microsoft account").size(11.5));
                            });
                        }
                        None => {
                            avatar(ui, "?", 30.0);
                            ui.vertical(|ui| {
                                ui.label(RichText::new("Not signed in").color(TEXT));
                                if ui.link(RichText::new("Sign in").size(12.0)).clicked() {
                                    self.tab = Tab::Accounts;
                                }
                            });
                        }
                    });
                },
            );
        });
    }

    fn view_play(&mut self, ui: &mut Ui) {
        theme::title(ui, "Play", "");

        let native = self.selected_instance().is_some_and(|i| i.engine == Engine::Native);
        if self.accounts.active().is_none() && !native {
            let text = if self.core.auth.is_some() {
                "Sign in with a Microsoft account that owns Minecraft: Java Edition to play."
            } else {
                "Microsoft sign-in is not configured in this build, so the game cannot be launched. See docs/AUTHENTICATION.md."
            };
            callout(ui, WARNING, text, |ui| {
                if ui.add(theme::secondary("Go to Accounts")).clicked() {
                    self.tab = Tab::Accounts;
                }
            });
        }

        if let Some(outcome) = self.last_outcome.clone() {
            let color = if outcome.success { SUCCESS } else { DANGER };
            callout(ui, color, &outcome.summary(), |ui| {
                if ui.add(theme::secondary("Open log")).clicked() {
                    open_path(self, &outcome.log_path);
                }
                if let Some(report) = &outcome.crash_report
                    && ui.add(theme::secondary("Open crash report")).clicked()
                {
                    open_path(self, report);
                }
                if ui.add(theme::secondary("Dismiss")).clicked() {
                    self.last_outcome = None;
                    *self.core.last_outcome.lock().unwrap() = None;
                }
            });
        }

        let Some(inst) = self.selected_instance().cloned() else {
            theme::card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                theme::section(ui, "Create your first instance");
                ui.label(muted("An instance is a game version with its own worlds, settings and mods."));
                ui.add_space(6.0);
                if ui.add(theme::primary("New instance")).clicked() {
                    self.open_new_instance();
                }
            });
            ui.add_space(14.0);
            self.whats_new(ui);
            return;
        };

        hero_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(muted("INSTANCE").size(11.5));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if self.instances.len() > 1 && !self.busy() {
                        let names: Vec<String> = self.instances.iter().map(|i| i.name.clone()).collect();
                        let mut chosen = inst.name.clone();
                        ComboBox::from_id_salt("instance-picker").selected_text(&chosen).show_ui(ui, |ui| {
                            for name in &names {
                                ui.selectable_value(&mut chosen, name.clone(), name);
                            }
                        });
                        if chosen != inst.name {
                            self.select_instance(&chosen);
                        }
                    }
                });
            });
            ui.label(RichText::new(&inst.name).family(theme::semibold()).size(30.0).color(TEXT));
            ui.horizontal(|ui| {
                theme::badge(ui, &format!("Minecraft {}", inst.game_version), ACCENT);
                theme::badge(ui, &inst.loader_label(), MUTED);
                if self.is_installed(&inst.game_version) {
                    theme::badge(ui, "Downloaded", SUCCESS);
                }
            });
            let last =
                inst.last_played.map_or_else(|| "Never played".into(), |t| format!("Last played {}", format_ago(t)));
            ui.label(muted(last));
            ui.add_space(14.0);
            self.play_controls(ui);
        });

        ui.add_space(14.0);
        self.whats_new(ui);
    }

    fn play_controls(&mut self, ui: &mut Ui) {
        match &self.play {
            PlayState::Idle => {
                let native = self.selected_instance().is_some_and(|i| i.engine == Engine::Native);
                let can_play = native || self.accounts.active().is_some();
                let button = ui.add_enabled(can_play, theme::primary("Play"));
                if button.clicked() {
                    self.start_launch(false);
                }
                if !can_play {
                    ui.label(muted("Sign in to enable Play."));
                }
            }
            PlayState::Preparing { stage, progress, .. } => {
                let fraction = progress.fraction();
                let (done, total) =
                    (progress.bytes_done.load(Ordering::Relaxed), progress.bytes_total.load(Ordering::Relaxed));
                let (files_done, files_total) =
                    (progress.files_done.load(Ordering::Relaxed), progress.files_total.load(Ordering::Relaxed));
                ui.label(RichText::new(stage.label()).color(TEXT));
                ui.add(
                    ProgressBar::new(fraction).desired_height(10.0).fill(ACCENT).corner_radius(CornerRadius::same(5)),
                );
                if files_total > 0 {
                    ui.label(
                        muted(format!("{} of {} · {files_done} of {files_total} files", mb(done), mb(total)))
                            .size(12.5),
                    );
                }
                if ui.add(theme::secondary("Cancel")).clicked() {
                    self.cancel_launch();
                }
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            }
            PlayState::Starting => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Starting Minecraft…").color(TEXT));
                });
                ui.label(muted("This window closes when the game opens and returns when you quit."));
            }
            PlayState::Failed(error) => {
                let error = error.clone();
                ui.label(RichText::new(&error.message).color(DANGER));
                ui.horizontal(|ui| {
                    if ui.add(theme::primary("Try again")).clicked() {
                        self.start_launch(false);
                    }
                    if error.offline_possible && ui.add(theme::secondary("Play offline (singleplayer only)")).clicked()
                    {
                        self.start_launch(true);
                    }
                    if ui.add(theme::secondary("Dismiss")).clicked() {
                        self.play = PlayState::Idle;
                    }
                });
            }
        }
    }

    fn whats_new(&mut self, ui: &mut Ui) {
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section(ui, "What's new");
            if let Some(m) = &self.manifest {
                ui.label(muted(format!(
                    "Latest Minecraft release: {} · latest snapshot: {}",
                    m.latest.release, m.latest.snapshot
                )));
            }
            ui.add_space(4.0);
            for line in latest_notes(RELEASE_NOTES).lines() {
                if let Some(h) = line.strip_prefix("## ") {
                    ui.label(RichText::new(h).family(theme::semibold()).color(TEXT));
                } else if let Some(item) = line.strip_prefix("- ") {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("•").color(ACCENT));
                        ui.label(item);
                    });
                } else if !line.trim().is_empty() {
                    ui.label(line);
                }
            }
        });
    }

    fn view_instances(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| theme::title(ui, "Instances", "Each instance keeps its own worlds, settings and mods."));
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                if ui.add_enabled(!self.busy(), theme::primary("New instance")).clicked() {
                    self.open_new_instance();
                }
            });
        });
        if self.instances.is_empty() {
            ui.label(muted("No instances yet."));
        }
        let selected = self.selected_instance().map(|i| i.name.clone());
        for inst in self.instances.clone() {
            let is_selected = selected.as_ref() == Some(&inst.name);
            let frame =
                theme::card().stroke(Stroke::new(1.0, if is_selected { ACCENT.linear_multiply(0.7) } else { BORDER }));
            frame.show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&inst.name).family(theme::semibold()).size(16.5).color(TEXT));
                        ui.horizontal(|ui| {
                            theme::badge(ui, &inst.game_version, ACCENT);
                            theme::badge(ui, &inst.loader_label(), MUTED);
                            let worlds = inst.world_count(&self.core.dirs);
                            ui.label(muted(format!("{worlds} world{}", if worlds == 1 { "" } else { "s" })));
                        });
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add_enabled(!self.busy(), theme::secondary("Edit")).clicked() {
                            self.edit = Some(inst.clone());
                        }
                        if ui.add(theme::secondary("Open folder")).clicked()
                            && let Ok(dir) = inst.dir(&self.core.dirs)
                        {
                            open_path(self, &dir);
                        }
                        if is_selected {
                            theme::badge(ui, "Selected", SUCCESS);
                        } else if ui.add_enabled(!self.busy(), theme::secondary("Select")).clicked() {
                            self.select_instance(&inst.name);
                        }
                    });
                });
            });
            ui.add_space(4.0);
        }
    }

    pub fn open_new_instance(&mut self) {
        let snapshots = self.core.settings.lock().unwrap().show_snapshots;
        let version = self.manifest.as_ref().map(|m| m.latest.release.clone());
        self.new_instance = Some(NewInstance {
            name: String::new(),
            version,
            native: false,
            filter: String::new(),
            snapshots,
            fabric: false,
            loaders: None,
            loader_choice: None,
            view_distance: 12,
            graphics: Graphics::Fancy,
            error: None,
        });
    }

    fn new_instance_modal(&mut self, ctx: &egui::Context) {
        let Some(mut draft) = self.new_instance.take() else {
            return;
        };
        let mut keep_open = true;
        let mut create = false;
        let mut request_loaders = None;
        modal(ctx, "new-instance", |ui| {
            ui.set_width(516.0);
            theme::title(ui, "New instance", "");
            ui.label(muted("Name"));
            ui.add(TextEdit::singleline(&mut draft.name).hint_text("e.g. Survival 1.21").desired_width(f32::INFINITY));
            ui.add_space(6.0);

            ui.horizontal(|ui| {
                ui.label(muted("Version"));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.checkbox(&mut draft.snapshots, "Snapshots");
                    ui.add(TextEdit::singleline(&mut draft.filter).hint_text("Search").desired_width(160.0));
                });
            });
            match &self.manifest {
                Some(manifest) => {
                    Frame::new()
                        .fill(theme::BG)
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::same(6))
                        .show(ui, |ui| {
                            ScrollArea::vertical().max_height(220.0).auto_shrink([false, true]).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                let filter = draft.filter.trim().to_lowercase();
                                for v in manifest.supported() {
                                    let wanted = match v.kind {
                                        VersionKind::Release => true,
                                        VersionKind::Snapshot => draft.snapshots,
                                        _ => false,
                                    };
                                    if !wanted || (!filter.is_empty() && !v.id.to_lowercase().contains(&filter)) {
                                        continue;
                                    }
                                    let selected = draft.version.as_deref() == Some(v.id.as_str());
                                    let row = ui.horizontal(|ui| {
                                        let r = ui.selectable_label(selected, &v.id);
                                        ui.label(
                                            muted(format!(
                                                "{} · {}",
                                                v.kind.label(),
                                                &v.release_time[..10.min(v.release_time.len())]
                                            ))
                                            .size(12.0),
                                        );
                                        if self.is_installed(&v.id) {
                                            theme::badge(ui, "Downloaded", SUCCESS);
                                        }
                                        r
                                    });
                                    if row.inner.clicked() {
                                        draft.version = Some(v.id.clone());
                                    }
                                }
                            });
                        });
                    ui.label(muted("Versions before 1.8 are not supported.").size(12.0));
                }
                None => {
                    match &self.manifest_error {
                        Some(e) => ui.label(RichText::new(format!("Version list unavailable: {e}")).color(DANGER)),
                        None => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(muted("Loading versions…"))
                            })
                            .inner
                        }
                    };
                    if self.manifest_error.is_some() && ui.add(theme::secondary("Retry")).clicked() {
                        self.manifest_error = None;
                        self.refresh_manifest();
                    }
                }
            }
            ui.add_space(6.0);

            if draft.version.as_deref() == Some(NATIVE_VERSION) {
                ui.label(muted("Engine"));
                ui.horizontal(|ui| {
                    ui.radio_value(&mut draft.native, false, "Official Java client");
                    ui.radio_value(&mut draft.native, true, "RustCraft engine (experimental)");
                });
                if draft.native {
                    ui.label(
                        muted("Local creative worlds only; no multiplayer or mods yet. Works without signing in, using built-in textures unless the official game files are installed.")
                            .size(12.0),
                    );
                }
            } else {
                draft.native = false;
            }
            if !draft.native {
                ui.label(muted("Mod loader"));
                ui.horizontal(|ui| {
                    ui.radio_value(&mut draft.fabric, false, "None (vanilla)");
                    ui.radio_value(&mut draft.fabric, true, "Fabric");
                });
            }
            if draft.fabric
                && !draft.native
                && let Some(version) = draft.version.clone()
            {
                match &draft.loaders {
                    Some((v, Loaders::Loaded(list))) if *v == version => {
                        let current = draft.loader_choice.clone().unwrap_or_default();
                        ComboBox::from_id_salt("fabric-loader")
                            .selected_text(format!("Fabric Loader {current}"))
                            .show_ui(ui, |ui| {
                                for l in list.iter().take(30) {
                                    let label =
                                        if l.stable { l.version.clone() } else { format!("{} (beta)", l.version) };
                                    ui.selectable_value(&mut draft.loader_choice, Some(l.version.clone()), label);
                                }
                            });
                        if list.is_empty() {
                            ui.label(RichText::new(format!("Fabric does not support {version}.")).color(WARNING));
                        }
                    }
                    Some((v, Loaders::Failed(e))) if *v == version => {
                        ui.label(RichText::new(format!("Could not list Fabric versions: {e}")).color(DANGER));
                        if ui.add(theme::secondary("Retry")).clicked() {
                            request_loaders = Some(version);
                        }
                    }
                    Some((v, Loaders::Loading)) if *v == version => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(muted("Loading Fabric versions…"));
                        });
                    }
                    _ => request_loaders = Some(version),
                }
                ui.label(muted("Forge and NeoForge are not supported yet.").size(12.0));
            }
            ui.add_space(6.0);

            ui.label(muted("Initial game options"));
            ui.add(egui::Slider::new(&mut draft.view_distance, 2..=32).text("view distance (chunks)"));
            ui.horizontal(|ui| {
                ui.label("Graphics");
                ui.radio_value(&mut draft.graphics, Graphics::Fast, "Fast");
                ui.radio_value(&mut draft.graphics, Graphics::Fancy, "Fancy");
            });

            if let Some(e) = &draft.error {
                ui.label(RichText::new(e).color(DANGER));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add(theme::primary("Create")).clicked() {
                    create = true;
                }
                if ui.add(theme::secondary("Cancel")).clicked() {
                    keep_open = false;
                }
            });
        });
        if let Some(version) = request_loaders {
            draft.loaders = Some((version.clone(), Loaders::Loading));
            self.load_fabric_loaders(version);
        }
        if create {
            match self.create_instance(&draft) {
                Ok(name) => {
                    self.reload_instances();
                    self.select_instance(&name);
                    self.notify(Level::Info, format!("Created {name}."));
                    keep_open = false;
                }
                Err(e) => draft.error = Some(e),
            }
        }
        if keep_open && !ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.new_instance = Some(draft);
        }
    }

    fn create_instance(&self, draft: &NewInstance) -> Result<String, String> {
        validate_name(&draft.name)?;
        let version = draft.version.clone().ok_or("Choose a version.")?;
        let loader = if draft.fabric && !draft.native {
            Loader::Fabric { version: draft.loader_choice.clone().ok_or("Choose a Fabric Loader version.")? }
        } else {
            Loader::Vanilla
        };
        let mut inst = Instance::new(&draft.name, &version, loader);
        if draft.native {
            inst.engine = Engine::Native;
        }
        inst.create(&self.core.dirs, draft.view_distance, draft.graphics).map_err(|e| e.to_string())?;
        Ok(inst.name)
    }

    fn edit_modal(&mut self, ctx: &egui::Context) {
        let Some(mut draft) = self.edit.take() else {
            return;
        };
        let mut keep_open = true;
        let mut save = false;
        modal(ctx, "edit-instance", |ui| {
            ui.set_width(500.0);
            theme::title(
                ui,
                &format!("Edit {}", draft.name),
                &format!("Minecraft {} · {}", draft.game_version, draft.loader_label()),
            );
            theme::section(ui, "Memory");
            ui.add(egui::Slider::new(&mut draft.max_memory_mb, 512..=16384).step_by(256.0).text("maximum (MB)"));
            ui.add(egui::Slider::new(&mut draft.min_memory_mb, 256..=8192).step_by(256.0).text("initial (MB)"));
            if draft.min_memory_mb > draft.max_memory_mb {
                draft.min_memory_mb = draft.max_memory_mb;
            }
            ui.add_space(4.0);
            theme::section(ui, "Java");
            let mut java = draft.java_path.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
            ui.add(TextEdit::singleline(&mut java).hint_text("Automatic (recommended)").desired_width(f32::INFINITY));
            draft.java_path = (!java.trim().is_empty()).then(|| java.trim().into());
            ui.add(
                TextEdit::singleline(&mut draft.jvm_args)
                    .hint_text("Extra JVM arguments, e.g. -XX:+UseG1GC")
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(4.0);
            theme::section(ui, "Game");
            let mut custom = draft.resolution.is_some();
            ui.checkbox(&mut custom, "Custom window size");
            if custom {
                let (mut w, mut h) = draft.resolution.unwrap_or((1280, 720));
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut w).range(320..=7680).suffix(" px"));
                    ui.label("×");
                    ui.add(egui::DragValue::new(&mut h).range(240..=4320).suffix(" px"));
                });
                draft.resolution = Some((w, h));
            } else {
                draft.resolution = None;
            }
            let mut server = draft.server.clone().unwrap_or_default();
            ui.add(
                TextEdit::singleline(&mut server)
                    .hint_text("Join server on launch (host:port), optional")
                    .desired_width(f32::INFINITY),
            );
            draft.server = (!server.trim().is_empty()).then(|| server.trim().to_string());
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.add(theme::primary("Save")).clicked() {
                    save = true;
                }
                if ui.add(theme::secondary("Cancel")).clicked() {
                    keep_open = false;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(theme::danger("Delete…")).clicked() {
                        self.confirm_delete = Some(draft.name.clone());
                        keep_open = false;
                    }
                });
            });
        });
        if save {
            match draft.save(&self.core.dirs) {
                Ok(()) => {
                    self.reload_instances();
                    keep_open = false;
                }
                Err(e) => self.notify(Level::Error, format!("Could not save {}: {e}", draft.name)),
            }
        }
        if keep_open && !ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.edit = Some(draft);
        }
    }

    fn delete_modal(&mut self, ctx: &egui::Context) {
        let Some(name) = self.confirm_delete.clone() else {
            return;
        };
        let worlds = self.instances.iter().find(|i| i.name == name).map_or(0, |i| i.world_count(&self.core.dirs));
        let mut close = false;
        modal(ctx, "delete-instance", |ui| {
            ui.set_width(420.0);
            theme::title(ui, &format!("Delete {name}?"), "");
            ui.label(format!(
                "This permanently deletes the instance folder, including {worlds} world{}, its settings and mods.",
                if worlds == 1 { "" } else { "s" }
            ));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add(theme::danger("Delete permanently")).clicked() {
                    match delete_instance(&self.core.dirs, &name) {
                        Ok(()) => self.notify(Level::Info, format!("Deleted {name}.")),
                        Err(e) => self.notify(Level::Error, format!("Could not delete {name}: {e}")),
                    }
                    self.reload_instances();
                    close = true;
                }
                if ui.add(theme::secondary("Keep it")).clicked() {
                    close = true;
                }
            });
        });
        if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.confirm_delete = None;
        }
    }

    fn view_accounts(&mut self, ui: &mut Ui) {
        theme::title(ui, "Accounts", "Sign in with the Microsoft account that owns Minecraft: Java Edition.");
        match &self.sign_in {
            SignIn::Waiting { url, cancel } => {
                let (url, cancel) = (url.clone(), cancel.clone());
                theme::card().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new("Finish signing in in your browser…").color(TEXT));
                    });
                    if let Some(url) = url {
                        ui.label(muted("If no browser window opened, open this link:"));
                        ui.horizontal(|ui| {
                            ui.hyperlink_to("Microsoft sign-in page", &url);
                            if ui.add(theme::secondary("Copy link")).clicked() {
                                ui.ctx().copy_text(url.clone());
                            }
                        });
                    }
                    if ui.add(theme::secondary("Cancel")).clicked() {
                        cancel.cancel();
                    }
                });
            }
            SignIn::Idle => {
                let configured = self.core.auth.is_some();
                if ui.add_enabled(configured, theme::primary("Add Microsoft account")).clicked() {
                    self.start_sign_in();
                }
                if !configured {
                    ui.label(RichText::new(rustcraft_auth::AuthError::NotConfigured.to_string()).color(WARNING));
                }
            }
        }
        ui.add_space(10.0);
        for record in self.accounts.records.clone() {
            let active = self.accounts.active.as_deref() == Some(record.id.as_str());
            theme::card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    avatar(ui, &record.name, 40.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&record.name).family(theme::semibold()).size(16.0).color(TEXT));
                        ui.label(
                            muted(format!(
                                "Ownership verified {} · {}",
                                format_ago(record.verified_at),
                                short_uuid(&record.id)
                            ))
                            .size(12.5),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add_enabled(!self.busy(), theme::danger("Sign out")).clicked() {
                            self.update_accounts(AccountChange::Remove(record.id.clone()));
                        }
                        if active {
                            theme::badge(ui, "Active", SUCCESS);
                        } else if ui.add_enabled(!self.busy(), theme::secondary("Use")).clicked() {
                            self.update_accounts(AccountChange::Activate(record.id.clone()));
                        }
                    });
                });
            });
            ui.add_space(4.0);
        }
        ui.add_space(8.0);
        ui.label(muted(
            "RustCraft never sees your password: you sign in on Microsoft's own page. Only a renewable sign-in \
             credential is kept, in your system's credential store. Sign out removes it.",
        ));
    }

    fn view_settings(&mut self, ui: &mut Ui) {
        theme::title(ui, "Settings", "");
        let mut settings = self.core.settings.lock().unwrap().clone();
        let before = settings.clone();

        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section(ui, "Appearance");
            if ui.checkbox(&mut settings.reduced_motion, "Reduce motion").changed() {
                let reduced = settings.reduced_motion;
                ui.ctx().all_styles_mut(|s| s.animation_time = if reduced { 0.0 } else { 0.12 });
            }
            ui.checkbox(&mut settings.show_snapshots, "Show snapshots in the version list by default");
        });
        ui.add_space(8.0);

        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section(ui, "Java");
            ui.label(muted(
                "Automatic uses the Java runtime Mojang publishes for each version, or a matching installed Java.",
            ));
            ui.horizontal(|ui| {
                let edit =
                    ui.add(TextEdit::singleline(&mut self.java_path_text).hint_text("Automatic").desired_width(420.0));
                if edit.changed() {
                    let t = self.java_path_text.trim();
                    settings.java_path = (!t.is_empty()).then(|| t.into());
                }
                if ui.add(theme::secondary("Use automatic")).clicked() {
                    self.java_path_text.clear();
                    settings.java_path = None;
                }
            });
            match &self.java {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(muted("Looking for installed Java…"));
                    });
                }
                Some(found) if found.is_empty() => {
                    ui.label(muted("No installed Java found."));
                }
                Some(found) => {
                    for j in found.clone() {
                        ui.horizontal(|ui| {
                            theme::badge(ui, &format!("Java {}", j.major), if j.is_64bit { ACCENT } else { WARNING });
                            ui.label(muted(j.path.display().to_string()).size(12.5));
                            if ui.small_button("Use").clicked() {
                                self.java_path_text = j.path.display().to_string();
                                settings.java_path = Some(j.path.clone());
                            }
                        });
                    }
                }
            }
            if ui.add(theme::secondary("Rescan")).clicked() {
                self.java = None;
                self.discover_java();
            }
        });
        ui.add_space(8.0);

        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section(ui, "Storage");
            ui.label(muted(format!("Data folder: {}", self.core.dirs.root().display())));
            ui.horizontal(|ui| {
                if ui.add(theme::secondary("Open data folder")).clicked() {
                    let root = self.core.dirs.root().to_path_buf();
                    open_path(self, &root);
                }
                if ui.add_enabled(!self.busy(), theme::secondary("Clear temporary files")).clicked() {
                    let mut freed = 0u64;
                    for dir in [self.core.dirs.cache(), self.core.dirs.natives()] {
                        freed += dir_size(&dir);
                        let _ = std::fs::remove_dir_all(&dir);
                    }
                    self.notify(Level::Info, format!("Cleared {} of temporary files.", mb(freed)));
                }
            });
            ui.label(muted(
                "Worlds, settings and mods live in each instance folder and are never touched by clearing.",
            ));
        });
        ui.add_space(8.0);

        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section(ui, "Privacy");
            ui.label(
                "RustCraft has no telemetry. It never uploads your worlds, mods, logs or crash reports. \
                 Network traffic goes only to Microsoft, Xbox and Mojang services for sign-in and game files, \
                 and to Fabric's metadata service if you use Fabric.",
            );
        });

        if settings != before {
            *self.core.settings.lock().unwrap() = settings;
            self.core.save_settings();
        }
    }

    fn view_diagnostics(&mut self, ui: &mut Ui) {
        theme::title(ui, "Help & diagnostics", "");
        let dirs = self.core.dirs.clone();
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section(ui, "System");
            egui::Grid::new("sys").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
                ui.label(muted("RustCraft"));
                ui.label(env!("CARGO_PKG_VERSION"));
                ui.end_row();
                ui.label(muted("Platform"));
                ui.label(format!("{} {}", std::env::consts::OS, std::env::consts::ARCH));
                ui.end_row();
                ui.label(muted("Data folder"));
                ui.add(egui::Label::new(dirs.root().display().to_string()).wrap());
                ui.end_row();
                ui.label(muted("Microsoft sign-in"));
                ui.label(if self.core.auth.is_some() { "Configured" } else { "Not configured in this build" });
                ui.end_row();
                ui.label(muted("Installed Java"));
                ui.label(match &self.java {
                    Some(f) if !f.is_empty() => {
                        f.iter().map(|j| format!("Java {}", j.major)).collect::<Vec<_>>().join(", ")
                    }
                    Some(_) => "None found".into(),
                    None => "Scanning…".into(),
                });
                ui.end_row();
            });
        });
        ui.add_space(8.0);

        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::section(ui, "Recent logs");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(theme::secondary("Open logs folder")).clicked() {
                        open_path(self, &dirs.logs());
                    }
                });
            });
            let mut logs = newest_files(&dirs.logs(), |n| n.ends_with(".log"), 8);
            let mut reports = Vec::new();
            for inst in &self.instances {
                if let Ok(dir) = inst.dir(&dirs) {
                    reports.extend(newest_files(&dir.join("crash-reports"), |n| n.ends_with(".txt"), 5));
                }
            }
            reports.sort_by_key(|r| std::cmp::Reverse(r.1));
            logs.truncate(8);
            if logs.is_empty() {
                ui.label(muted("No logs yet."));
            }
            for (path, _) in logs {
                file_row(self, ui, &path);
            }
            ui.add_space(6.0);
            theme::section(ui, "Crash reports");
            if reports.is_empty() {
                ui.label(muted("No crash reports."));
            }
            for (path, _) in reports.into_iter().take(8) {
                file_row(self, ui, &path);
            }
        });
        ui.add_space(8.0);

        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section(ui, "What runs where");
            egui::Grid::new("compat").num_columns(3).spacing([18.0, 8.0]).striped(true).show(ui, |ui| {
                ui.label(RichText::new("Mode").family(theme::semibold()));
                ui.label(RichText::new("Versions").family(theme::semibold()));
                ui.label(RichText::new("Status").family(theme::semibold()));
                ui.end_row();
                for (mode, versions, status, color) in COMPATIBILITY {
                    ui.label(*mode);
                    ui.label(muted(*versions));
                    theme::badge(ui, status, *color);
                    ui.end_row();
                }
            });
            ui.label(muted("Details and known issues: docs/COMPATIBILITY.md and docs/TROUBLESHOOTING.md in the RustCraft repository."));
        });
    }

    fn toasts(&mut self, ctx: &egui::Context) {
        self.notices.retain(|n| match n.level {
            Level::Info => n.at.elapsed() < Duration::from_secs(6),
            Level::Warn => n.at.elapsed() < Duration::from_secs(12),
            Level::Error => true,
        });
        if self.notices.is_empty() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(500));
        let mut dismiss = None;
        egui::Area::new(Id::unique("toasts")).anchor(Align2::RIGHT_BOTTOM, Vec2::new(-20.0, -20.0)).show(ctx, |ui| {
            ui.set_max_width(380.0);
            for (i, n) in self.notices.iter().enumerate().rev().take(4) {
                let color = match n.level {
                    Level::Info => SUCCESS,
                    Level::Warn => WARNING,
                    Level::Error => DANGER,
                };
                Frame::new()
                    .fill(CARD_RAISED)
                    .stroke(Stroke::new(1.0, color.linear_multiply(0.6)))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::same(12))
                    .shadow(egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(90) })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (dot, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                            ui.painter().circle_filled(dot.center(), 4.0, color);
                            ui.add(egui::Label::new(RichText::new(&n.text).color(TEXT)).wrap());
                            if close_button(ui).on_hover_text("Dismiss").clicked() {
                                dismiss = Some(i);
                            }
                        });
                    });
                ui.add_space(6.0);
            }
        });
        if let Some(i) = dismiss {
            self.notices.remove(i);
        }
    }
}

const COMPATIBILITY: &[(&str, &str, &str, Color32)] = &[
    ("Official client (JVM)", "1.8 – latest release/snapshot", "Implemented", SUCCESS),
    ("Fabric mods (JVM)", "Versions Fabric supports", "Implemented", SUCCESS),
    ("Forge / NeoForge mods", "—", "Not supported", DANGER),
    ("RustCraft engine", "1.8.9 local worlds (creative)", "Experimental", WARNING),
    ("RustCraft engine multiplayer", "—", "Not yet available", WARNING),
];

fn modal(ctx: &egui::Context, id: &str, add: impl FnOnce(&mut Ui)) {
    Modal::new(Id::unique(id))
        .frame(theme::card().inner_margin(Margin::same(24)).fill(CARD))
        .backdrop_color(Color32::from_black_alpha(150))
        .show(ctx, |ui| {
            let height = (ctx.content_rect().height() - 140.0).max(200.0);
            ScrollArea::vertical().max_height(height).min_scrolled_height(height).show(ui, add);
        });
}

fn hero_frame() -> Frame {
    theme::card()
        .fill(Color32::from_rgb(0x23, 0x21, 0x22))
        .stroke(Stroke::new(1.0, ACCENT.linear_multiply(0.35)))
        .inner_margin(Margin::same(24))
        .corner_radius(CornerRadius::same(16))
}

fn callout(ui: &mut Ui, color: Color32, text: &str, buttons: impl FnOnce(&mut Ui)) {
    Frame::new()
        .fill(color.linear_multiply(0.10))
        .stroke(Stroke::new(1.0, color.linear_multiply(0.45)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).color(TEXT));
            ui.horizontal(buttons);
        });
    ui.add_space(10.0);
}

/// A left-aligned navigation row with keyboard focus and screen-reader info.
fn nav_item(ui: &mut Ui, selected: bool, icon: Icon, label: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 38.0), Sense::click());
    response.widget_info(|| WidgetInfo::selected(egui::Role::Button, true, selected, label));
    let hover = ui.ctx().animate_bool(response.id, response.hovered() || selected);
    let fill = if selected { CARD_RAISED } else { CARD.linear_multiply(hover) };
    let painter = ui.painter();
    painter.rect_filled(rect, 8.0, fill);
    if selected {
        painter.rect_filled(Rect::from_min_size(rect.min + Vec2::new(0.0, 10.0), Vec2::new(3.0, 18.0)), 2.0, ACCENT);
    }
    if response.has_focus() {
        painter.rect_stroke(rect, 8.0, Stroke::new(1.5, ACCENT), StrokeKind::Inside);
    }
    let color = if selected { TEXT } else { MUTED.lerp_to_gamma(TEXT, hover * 0.5) };
    paint_icon(painter, rect.left_center() + Vec2::new(22.0, 0.0), icon, if selected { ACCENT } else { color });
    painter.text(
        rect.left_center() + Vec2::new(42.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(14.5),
        color,
    );
    response
}

#[derive(Clone, Copy)]
enum Icon {
    Play,
    Grid,
    Person,
    Sliders,
    Help,
    Close,
}

/// Small vector icons, so no icon font or image assets are needed.
fn paint_icon(p: &egui::Painter, c: egui::Pos2, icon: Icon, color: Color32) {
    let v = Vec2::new;
    match icon {
        Icon::Play => {
            p.add(egui::Shape::convex_polygon(
                vec![c + v(-4.5, -6.5), c + v(6.5, 0.0), c + v(-4.5, 6.5)],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Grid => {
            for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                p.rect_filled(Rect::from_center_size(c + v(dx * 3.6, dy * 3.6), v(5.6, 5.6)), 1.5, color);
            }
        }
        Icon::Person => {
            p.circle_filled(c + v(0.0, -3.8), 3.6, color);
            p.rect_filled(Rect::from_center_size(c + v(0.0, 4.6), v(12.0, 5.6)), 3.0, color);
        }
        Icon::Sliders => {
            for (dy, knob) in [(-4.5, 3.0), (0.0, -3.0), (4.5, 1.5)] {
                p.line_segment([c + v(-6.5, dy), c + v(6.5, dy)], Stroke::new(1.6, color));
                p.circle_filled(c + v(knob, dy), 2.4, color);
            }
        }
        Icon::Help => {
            p.circle_stroke(c, 6.8, Stroke::new(1.5, color));
            p.text(c + v(0.0, 0.5), Align2::CENTER_CENTER, "?", FontId::new(10.5, theme::semibold()), color);
        }
        Icon::Close => {
            p.line_segment([c + v(-4.0, -4.0), c + v(4.0, 4.0)], Stroke::new(1.6, color));
            p.line_segment([c + v(4.0, -4.0), c + v(-4.0, 4.0)], Stroke::new(1.6, color));
        }
    }
}

fn close_button(ui: &mut Ui) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, "Dismiss"));
    let color = if response.hovered() { TEXT } else { MUTED };
    paint_icon(ui.painter(), rect.center(), Icon::Close, color);
    response
}

/// Brand mark: an ember block with an inset face.
fn logo(ui: &mut Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, size * 0.24, ACCENT);
    let inner = rect.shrink(size * 0.24);
    p.rect_filled(Rect::from_min_max(inner.min, inner.center()), 2.0, Color32::from_black_alpha(55));
    p.rect_filled(Rect::from_min_max(inner.center(), inner.max), 2.0, Color32::from_black_alpha(55));
    p.rect_stroke(inner, 3.0, Stroke::new(1.5, Color32::from_white_alpha(140)), StrokeKind::Inside);
}

fn avatar(ui: &mut Ui, name: &str, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let hue = name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) % 360;
    let color = egui::ecolor::Hsva::new(hue as f32 / 360.0, 0.45, 0.62, 1.0);
    ui.painter().circle_filled(rect.center(), size / 2.0, Color32::from(color));
    let initial = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        initial,
        FontId::new(size * 0.45, theme::semibold()),
        Color32::WHITE,
    );
}

fn file_row(app: &mut LauncherApp, ui: &mut Ui, path: &Path) {
    ui.horizontal(|ui| {
        ui.label(muted(path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).size(12.5));
        if ui.small_button("Open").clicked() {
            open_path(app, path);
        }
    });
}

fn open_path(app: &mut LauncherApp, path: &Path) {
    let result =
        if path.is_dir() || !path.exists() { rustcraft_platform::open_folder(path) } else { open::that_detached(path) };
    if let Err(e) = result {
        app.notify(Level::Error, format!("Could not open {}: {e}", path.display()));
    }
}

fn newest_files(
    dir: &Path,
    keep: impl Fn(&str) -> bool,
    limit: usize,
) -> Vec<(std::path::PathBuf, std::time::SystemTime)> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| keep(&e.file_name().to_string_lossy()))
        .filter_map(|e| Some((e.path(), e.metadata().ok()?.modified().ok()?)))
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.1));
    files.truncate(limit);
    files
}

fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            _ => e.metadata().map(|m| m.len()).unwrap_or(0),
        })
        .sum()
}

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
}

fn short_uuid(id: &str) -> String {
    id.get(..8).map(|s| format!("{s}…")).unwrap_or_else(|| id.into())
}

/// The newest section of CHANGELOG.md (from the first `## ` heading to the next).
fn latest_notes(changelog: &str) -> &str {
    let start = changelog.find("\n## ").map_or(0, |i| i + 1);
    let rest = &changelog[start..];
    let end = rest[3..].find("\n## ").map_or(rest.len(), |i| i + 3);
    &rest[..end]
}

#[cfg(test)]
mod tests {
    use super::latest_notes;

    #[test]
    fn release_notes_section() {
        let c = "# Changelog\n\n## 0.2.0\n- b\n\n## 0.1.0\n- a\n";
        assert_eq!(latest_notes(c), "## 0.2.0\n- b\n");
    }
}
