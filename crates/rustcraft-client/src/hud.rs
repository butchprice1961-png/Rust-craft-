//! In-game interface drawn with egui: crosshair, hotbar, debug screen,
//! loading screen, the pause/options menu and, on servers, chat, health
//! and the connection screens.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use egui::{Align2, Color32, CornerRadius, FontId, Frame, Margin, Pos2, RichText, Stroke, Vec2};

use rustcraft_renderer::{Stats, TextureData};
use rustcraft_world::block::{self, BlockState, Face, Tint};

use crate::game::Game;
use crate::remote::Status;
use crate::settings::Settings;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Menu {
    None,
    Pause,
    Options,
    Chat,
}

pub enum Action {
    None,
    Resume,
    SaveAndQuit,
    SettingsChanged,
    SendChat(String),
    Respawn,
    /// Close after the disconnect screen.
    Quit,
}

pub struct Hud {
    pub menu: Menu,
    pub show_debug: bool,
    pub hidden: bool,
    pub chat_input: String,
    /// Name tags of players on a server, in screen points.
    pub labels: Vec<(Pos2, String)>,
    /// Set when the chat opens: the key that opened it may arrive as text.
    pub chat_fresh: bool,
    icons: HashMap<BlockState, egui::TextureHandle>,
    frames: VecDeque<(Instant, Duration)>,
    pub adapter: String,
}

/// How long chat lines stay on screen while the chat is closed.
const CHAT_FADE: Duration = Duration::from_secs(10);

impl Hud {
    pub fn new(adapter: String) -> Self {
        Self {
            menu: Menu::None,
            show_debug: false,
            hidden: false,
            chat_input: String::new(),
            labels: Vec::new(),
            chat_fresh: false,
            icons: HashMap::new(),
            frames: VecDeque::new(),
            adapter,
        }
    }

    pub fn record_frame(&mut self, dt: Duration) {
        let now = Instant::now();
        self.frames.push_back((now, dt));
        while self.frames.front().is_some_and(|(t, _)| now - *t > Duration::from_secs(1)) {
            self.frames.pop_front();
        }
    }

    pub fn fps(&self) -> usize {
        self.frames.len()
    }

    fn icon(&mut self, ctx: &egui::Context, textures: &TextureData, state: BlockState) -> egui::TextureId {
        self.icons
            .entry(state)
            .or_insert_with(|| {
                let face = if matches!(state.props().shape, block::Shape::Cross | block::Shape::Torch) {
                    Face::North
                } else {
                    Face::Up
                };
                let (name, tint) = block::texture(state, face);
                let layer = textures.lookup.get(name).copied().unwrap_or(0) as usize;
                let tint = match tint {
                    Tint::Grass => 0x7FB238,
                    Tint::Foliage => 0x48B518,
                    Tint::Rgb(c) => c,
                    _ => 0xFFFFFF,
                };
                let s = textures.size as usize;
                let pixels: Vec<Color32> = textures.layers[layer]
                    .chunks(4)
                    .map(|p| {
                        let ch = |v: u8, shift: u32| (v as u32 * ((tint >> shift) & 255) / 255) as u8;
                        Color32::from_rgba_unmultiplied(ch(p[0], 16), ch(p[1], 8), ch(p[2], 0), p[3])
                    })
                    .collect();
                let image = egui::ColorImage::new([s, s], pixels);
                ctx.load_texture(format!("icon-{}", state.0), image, egui::TextureOptions::NEAREST)
            })
            .id()
    }

    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        game: &mut Game,
        textures: &TextureData,
        stats: Stats,
        settings: &mut Settings,
    ) -> Action {
        if let Some(r) = game.remote_ref() {
            match &r.status {
                Status::Connecting => {
                    Self::waiting(ctx, &format!("Connecting to {}", r.address));
                    return Action::None;
                }
                Status::Closed(reason) => return Self::disconnected(ctx, reason),
                Status::Playing => {}
            }
        }
        // Like vanilla: wait for the ground under the player, then stream the
        // rest in while playing.
        if !game.spawn_area_ready() {
            Self::waiting(ctx, if game.remote_ref().is_some() { "Joining world" } else { "Loading world" });
            return Action::None;
        }
        if !self.hidden {
            self.crosshair(ctx);
            self.hotbar(ctx, game, textures);
            self.server_status(ctx, game);
        }
        if self.show_debug {
            self.debug(ctx, game, stats);
        }
        if let Some(r) = game.remote_ref()
            && r.health <= 0.0
            && !r.creative()
        {
            return Self::death_screen(ctx);
        }
        match self.menu {
            Menu::None => Action::None,
            Menu::Pause => self.pause_menu(ctx, game.remote_ref().is_some()),
            Menu::Options => self.options(ctx, settings),
            Menu::Chat => self.chat_input(ctx),
        }
    }

    fn waiting(ctx: &egui::Context, text: &str) {
        egui::Area::new(egui::Id::unique("loading")).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
            Frame::new()
                .fill(Color32::from_black_alpha(200))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(Margin::same(24))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(22.0).color(Color32::WHITE));
                        ui.label(RichText::new(text).size(22.0).color(Color32::WHITE));
                    });
                });
        });
    }

    fn disconnected(ctx: &egui::Context, reason: &str) -> Action {
        let mut action = Action::None;
        Self::menu_frame(ctx, "Disconnected", |ui| {
            ui.label(RichText::new(reason).color(Color32::LIGHT_GRAY));
            ui.add_space(8.0);
            if ui.add(egui::Button::new("Quit").min_size(Vec2::new(0.0, 34.0))).clicked() {
                action = Action::Quit;
            }
        });
        action
    }

    fn death_screen(ctx: &egui::Context) -> Action {
        let mut action = Action::None;
        Self::menu_frame(ctx, "You died!", |ui| {
            if ui.add(egui::Button::new("Respawn").min_size(Vec2::new(0.0, 34.0))).clicked() {
                action = Action::Respawn;
            }
        });
        action
    }

    /// Chat, the action bar, health and digging progress on a server.
    fn server_status(&self, ctx: &egui::Context, game: &Game) {
        let Some(r) = game.remote_ref() else { return };
        let screen = ctx.content_rect();
        let open = self.menu == Menu::Chat;
        let lines: Vec<&String> = r
            .chat
            .iter()
            .rev()
            .filter(|(at, _)| open || at.elapsed() < CHAT_FADE)
            .take(if open { 20 } else { 10 })
            .map(|(_, line)| line)
            .collect();
        if !lines.is_empty() {
            egui::Area::new(egui::Id::unique("chat")).anchor(Align2::LEFT_BOTTOM, Vec2::new(8.0, -96.0)).show(
                ctx,
                |ui| {
                    ui.set_max_width(screen.width().min(620.0));
                    for line in lines.into_iter().rev() {
                        Frame::new().fill(Color32::from_black_alpha(110)).inner_margin(Margin::symmetric(6, 1)).show(
                            ui,
                            |ui| {
                                ui.label(RichText::new(line).color(Color32::WHITE));
                            },
                        );
                    }
                },
            );
        }
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("status")));
        for (pos, name) in &self.labels {
            let galley = painter.layout_no_wrap(name.clone(), FontId::proportional(14.0), Color32::WHITE);
            let rect = Align2::CENTER_BOTTOM.anchor_size(*pos, galley.size()).expand(3.0);
            painter.rect_filled(rect, 2.0, Color32::from_black_alpha(110));
            painter.galley(rect.shrink(3.0).min, galley, Color32::WHITE);
        }
        if let Some((at, text)) = &r.action_bar
            && at.elapsed() < Duration::from_secs(3)
        {
            let pos = screen.center_bottom() - Vec2::new(0.0, 100.0);
            painter.text(pos, Align2::CENTER_BOTTOM, text, FontId::proportional(15.0), Color32::WHITE);
        }
        if !r.creative() {
            let text = format!("Health {:.0} / 20    Food {} / 20", r.health.max(0.0), r.food);
            let pos = screen.center_bottom() - Vec2::new(0.0, 76.0);
            painter.text(
                pos,
                Align2::CENTER_BOTTOM,
                text,
                FontId::proportional(14.0),
                Color32::from_rgb(0xff, 0xd0, 0xd0),
            );
        }
        if let Some(progress) = r.dig_progress() {
            let c = screen.center() + Vec2::new(-20.0, 16.0);
            let bar = egui::Rect::from_min_size(c, Vec2::new(40.0, 4.0));
            painter.rect_filled(bar, 1.0, Color32::from_black_alpha(160));
            painter.rect_filled(
                egui::Rect::from_min_size(c, Vec2::new(40.0 * progress.min(1.0), 4.0)),
                1.0,
                Color32::WHITE,
            );
        }
    }

    fn chat_input(&mut self, ctx: &egui::Context) -> Action {
        let mut action = Action::None;
        egui::Area::new(egui::Id::unique("chat-input"))
            .anchor(Align2::LEFT_BOTTOM, Vec2::new(8.0, -64.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let prefix = self.chat_fresh.then(|| self.chat_input.clone());
                let edit = egui::TextEdit::singleline(&mut self.chat_input)
                    .desired_width(ctx.content_rect().width().min(620.0))
                    .char_limit(100)
                    .hint_text("Chat or /command, Enter to send");
                let response = ui.add(edit);
                response.request_focus();
                if let Some(prefix) = prefix {
                    self.chat_input = prefix;
                    self.chat_fresh = false;
                }
                if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    action = Action::SendChat(std::mem::take(&mut self.chat_input));
                }
            });
        action
    }

    fn crosshair(&self, ctx: &egui::Context) {
        let c = ctx.content_rect().center();
        let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Background, egui::Id::unique("crosshair")));
        let stroke = Stroke::new(2.0, Color32::from_white_alpha(220));
        p.line_segment([c - Vec2::new(9.0, 0.0), c + Vec2::new(9.0, 0.0)], stroke);
        p.line_segment([c - Vec2::new(0.0, 9.0), c + Vec2::new(0.0, 9.0)], stroke);
    }

    fn hotbar(&mut self, ctx: &egui::Context, game: &Game, textures: &TextureData) {
        let slot = 44.0;
        let icons: Vec<egui::TextureId> = game.hotbar.iter().map(|s| self.icon(ctx, textures, *s)).collect();
        egui::Area::new(egui::Id::unique("hotbar")).anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -8.0)).show(
            ctx,
            |ui| {
                Frame::new()
                    .fill(Color32::from_black_alpha(150))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::same(4))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            for (i, icon) in icons.iter().enumerate() {
                                let (rect, _) = ui.allocate_exact_size(Vec2::splat(slot), egui::Sense::hover());
                                let selected = i == game.selected;
                                ui.painter().rect_filled(
                                    rect,
                                    4.0,
                                    Color32::from_white_alpha(if selected { 60 } else { 18 }),
                                );
                                egui::Image::new((*icon, Vec2::splat(slot - 14.0))).paint_at(ui, rect.shrink(7.0));
                                if selected {
                                    ui.painter().rect_stroke(
                                        rect,
                                        4.0,
                                        Stroke::new(2.5, Color32::WHITE),
                                        egui::StrokeKind::Inside,
                                    );
                                }
                            }
                        });
                    });
            },
        );
        let name = game.hotbar[game.selected].props().name.replace('_', " ");
        let at = ctx.content_rect().center_bottom() - Vec2::new(0.0, slot + 30.0);
        ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("item-name"))).text(
            at,
            Align2::CENTER_BOTTOM,
            name,
            FontId::proportional(15.0),
            Color32::WHITE,
        );
    }

    fn debug(&self, ctx: &egui::Context, game: &Game, stats: Stats) {
        let p = &game.player;
        let b = p.pos.floor().as_ivec3();
        let facing = ["south (+Z)", "west (-X)", "north (-Z)", "east (+X)"]
            [((p.yaw / 90.0 + 0.5).floor() as i32).rem_euclid(4) as usize];
        let frame_ms = self.frames.iter().map(|(_, d)| d.as_secs_f64() * 1000.0).fold(0.0, f64::max);
        let lines = [
            format!("RustCraft {} (Minecraft 1.8.9 engine)", env!("CARGO_PKG_VERSION")),
            format!("{} fps (slowest frame {frame_ms:.1} ms)", self.fps()),
            format!("XYZ: {:.3} / {:.5} / {:.3}", p.pos.x, p.pos.y, p.pos.z),
            format!("Block: {} {} {}  Chunk: {} {}", b.x, b.y, b.z, b.x >> 4, b.z >> 4),
            format!("Facing: {facing} ({:.1} / {:.1})", p.yaw.rem_euclid(360.0), p.pitch),
            format!(
                "Light: sky {} block {}",
                game.world.light(rustcraft_world::LightKind::Sky, b),
                game.world.light(rustcraft_world::LightKind::Block, b)
            ),
            format!(
                "Chunks: {} loaded, sections {} drawn / {} meshed",
                game.world.chunk_count(),
                stats.sections_drawn,
                stats.sections_loaded
            ),
            format!("Triangles: {}  GPU memory: {:.1} MiB", stats.triangles, stats.gpu_bytes as f64 / 1048576.0),
            format!("Time: {} (day {})", game.day_time.rem_euclid(24000), game.day_time / 24000),
            format!("GPU: {}", self.adapter),
            game.remote_ref().map_or(String::new(), |r| format!("Server: {} ({:?} mode)", r.address, r.mode)),
            match game.target {
                Some(t) => format!(
                    "Looking at: {} {} {} {}",
                    game.world.block(t.block).props().name,
                    t.block.x,
                    t.block.y,
                    t.block.z
                ),
                None => String::new(),
            },
        ];
        egui::Area::new(egui::Id::unique("debug")).fixed_pos(Pos2::new(6.0, 6.0)).show(ctx, |ui| {
            for line in lines.iter().filter(|l| !l.is_empty()) {
                Frame::new().fill(Color32::from_black_alpha(120)).inner_margin(Margin::symmetric(4, 1)).show(
                    ui,
                    |ui| {
                        ui.label(RichText::new(line).monospace().color(Color32::WHITE));
                    },
                );
            }
        });
    }

    fn menu_frame(ctx: &egui::Context, title: &str, add: impl FnOnce(&mut egui::Ui)) {
        egui::Area::new(egui::Id::unique("menu-dim")).fixed_pos(Pos2::ZERO).order(egui::Order::Middle).show(
            ctx,
            |ui| {
                ui.painter().rect_filled(ctx.content_rect(), 0.0, Color32::from_black_alpha(120));
            },
        );
        egui::Area::new(egui::Id::unique("menu"))
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                Frame::new()
                    .fill(Color32::from_rgb(0x21, 0x23, 0x28))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(Margin::same(24))
                    .show(ui, |ui| {
                        ui.set_width(300.0);
                        ui.vertical_centered_justified(|ui| {
                            ui.label(RichText::new(title).size(22.0).color(Color32::WHITE));
                            ui.add_space(8.0);
                            add(ui);
                        });
                    });
            });
    }

    fn pause_menu(&mut self, ctx: &egui::Context, multiplayer: bool) -> Action {
        let mut action = Action::None;
        Self::menu_frame(ctx, "Game menu", |ui| {
            if ui.add(egui::Button::new("Back to game").min_size(Vec2::new(0.0, 34.0))).clicked() {
                action = Action::Resume;
            }
            if ui.add(egui::Button::new("Options…").min_size(Vec2::new(0.0, 34.0))).clicked() {
                self.menu = Menu::Options;
            }
            let leave = if multiplayer { "Disconnect" } else { "Save and quit" };
            if ui.add(egui::Button::new(leave).min_size(Vec2::new(0.0, 34.0))).clicked() {
                action = Action::SaveAndQuit;
            }
        });
        action
    }

    fn options(&mut self, ctx: &egui::Context, settings: &mut Settings) -> Action {
        let before = settings.clone();
        Self::menu_frame(ctx, "Options", |ui| {
            ui.add(egui::Slider::new(&mut settings.render_distance, 2..=32).text("render distance"));
            ui.add(egui::Slider::new(&mut settings.fov, 50.0..=110.0).text("field of view"));
            ui.add(egui::Slider::new(&mut settings.sensitivity, 0.1..=3.0).text("mouse sensitivity"));
            ui.checkbox(&mut settings.vsync, "VSync");
            ui.add(egui::Slider::new(&mut settings.max_fps, 30..=500).text("frame rate limit (without VSync)"));
            ui.add_space(6.0);
            if ui.add(egui::Button::new("Done").min_size(Vec2::new(0.0, 34.0))).clicked() {
                self.menu = Menu::Pause;
            }
        });
        if *settings != before { Action::SettingsChanged } else { Action::None }
    }
}
