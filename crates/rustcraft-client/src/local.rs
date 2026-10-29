//! A local (singleplayer) world on disk in the 1.8 Anvil format. Chunks
//! are loaded or generated and lit on background threads; modified chunks
//! are saved periodically, when unloaded and on exit.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use glam::DVec3;

use rustcraft_core::nbt::Compound;
use rustcraft_world::anvil::{self, LevelInfo, PlayerInfo};
use rustcraft_world::generate::Generator;
use rustcraft_world::{Chunk, World};

use crate::player::Player;

const SAVE_INTERVAL: Duration = Duration::from_secs(45);

pub struct LocalWorld {
    pub dir: PathBuf,
    pub info: LevelInfo,
    level_root: Option<Compound>,
    jobs: Sender<(i32, i32)>,
    results: Receiver<Chunk>,
    requested: HashSet<(i32, i32)>,
    modified: HashSet<(i32, i32)>,
    backed_up: HashSet<PathBuf>,
    last_save: Instant,
}

impl LocalWorld {
    /// Opens `saves/<name>`, creating a new world (with `seed`) if absent.
    pub fn open(saves: &Path, name: &str, seed: Option<i64>) -> anyhow::Result<LocalWorld> {
        let dir = saves.join(name);
        let (info, root) = if dir.join("level.dat").exists() {
            let (info, root) = anvil::read_level(&dir).with_context(|| format!("reading {}", dir.display()))?;
            // Survival rules (damage, drops, inventory) do not exist yet, and
            // playing such a world with creative rules would change it.
            if info.game_type != 1 {
                let mode = ["survival", "creative", "adventure", "spectator"].get(info.game_type as usize);
                anyhow::bail!(
                    "\"{name}\" is a {} world. The RustCraft engine only plays creative worlds so far; \
                     open it with the official client.",
                    mode.unwrap_or(&"non-creative")
                );
            }
            (info, Some(root))
        } else {
            std::fs::create_dir_all(&dir)?;
            let seed = seed.unwrap_or_else(|| rustcraft_core::fs::unix_time() as i64 ^ 0x5DEECE66D);
            let (x, y, z) = Generator::new(seed).spawn_point();
            let info = LevelInfo {
                name: name.into(),
                seed,
                spawn: [x, y, z],
                time: 0,
                day_time: 1000,
                game_type: 1,
                player: None,
            };
            anvil::write_level(&dir, &info, None)?;
            (info, None)
        };
        let generator = Arc::new(Generator::new(info.seed));
        let (jobs, job_rx) = channel::<(i32, i32)>();
        let (result_tx, results) = channel();
        let job_rx = Arc::new(Mutex::new(job_rx));
        let workers = std::thread::available_parallelism().map_or(2, |n| n.get().saturating_sub(1).clamp(1, 3));
        for i in 0..workers {
            let (job_rx, result_tx, generator, dir) =
                (job_rx.clone(), result_tx.clone(), generator.clone(), dir.clone());
            std::thread::Builder::new()
                .name(format!("chunk-loader-{i}"))
                .spawn(move || {
                    loop {
                        let job = job_rx.lock().unwrap().recv();
                        let Ok((cx, cz)) = job else { break };
                        let mut chunk = match anvil::load_chunk(&dir, cx, cz) {
                            Ok(Some(c)) => c,
                            Ok(None) => generator.generate(cx, cz),
                            Err(e) => {
                                // Never overwrite a damaged chunk with new terrain silently.
                                tracing::error!("chunk {cx},{cz} is damaged ({e}); showing it empty");
                                let mut c = Chunk::new(cx, cz);
                                c.extra = Some(Box::default());
                                c
                            }
                        };
                        if chunk.needs_light {
                            rustcraft_world::light::light_chunk_local(&mut chunk, true);
                        }
                        if result_tx.send(chunk).is_err() {
                            break;
                        }
                    }
                })
                .expect("spawn chunk loader");
        }
        Ok(LocalWorld {
            dir,
            info,
            level_root: root,
            jobs,
            results,
            requested: HashSet::new(),
            modified: HashSet::new(),
            backed_up: HashSet::new(),
            last_save: Instant::now(),
        })
    }

    pub fn spawn_position(&self) -> (DVec3, f32, f32, bool) {
        match &self.info.player {
            Some(p) => (DVec3::from_array(p.pos), p.yaw, p.pitch, p.flying),
            None => {
                let [x, y, z] = self.info.spawn;
                (DVec3::new(x as f64 + 0.5, y as f64, z as f64 + 0.5), 0.0, 0.0, false)
            }
        }
    }

    /// Requests chunks around `center` (nearest first), takes finished ones
    /// (at most `max_new` per call) and unloads distant chunks. Returns the
    /// positions of chunks added to the world.
    pub fn stream(&mut self, world: &mut World, center: (i32, i32), radius: i32, max_new: usize) -> Vec<(i32, i32)> {
        let mut wanted: Vec<(i32, i32)> = (-radius..=radius)
            .flat_map(|dx| (-radius..=radius).map(move |dz| (center.0 + dx, center.1 + dz)))
            .filter(|c| !world.is_loaded(c.0, c.1) && !self.requested.contains(c))
            .collect();
        wanted.sort_by_key(|(x, z)| (x - center.0).pow(2) + (z - center.1).pow(2));
        for c in wanted.into_iter().take(64) {
            if self.jobs.send(c).is_ok() {
                self.requested.insert(c);
            }
        }
        let mut added = Vec::new();
        while added.len() < max_new {
            let Ok(chunk) = self.results.try_recv() else { break };
            let pos = (chunk.x, chunk.z);
            self.requested.remove(&pos);
            let generated = chunk.extra.is_none();
            if (pos.0 - center.0).abs() > radius + 1 || (pos.1 - center.1).abs() > radius + 1 {
                continue;
            }
            world.insert_chunk(chunk);
            if generated {
                self.modified.insert(pos);
            }
            added.push(pos);
        }
        // Unload chunks well outside the radius, saving them first.
        let far: Vec<(i32, i32)> = world
            .chunks()
            .map(|c| (c.x, c.z))
            .filter(|(x, z)| (x - center.0).abs() > radius + 2 || (z - center.1).abs() > radius + 2)
            .collect();
        if !far.is_empty() {
            let dirty: Vec<&Chunk> =
                far.iter().filter(|p| self.modified.contains(p)).filter_map(|p| world.chunk(p.0, p.1)).collect();
            if let Err(e) = anvil::save_chunks(&self.dir, dirty, &mut self.backed_up) {
                tracing::error!("could not save unloading chunks: {e}");
                return added;
            }
            for p in far {
                self.modified.remove(&p);
                world.remove_chunk(p.0, p.1);
            }
        }
        added
    }

    pub fn mark_modified(&mut self, cx: i32, cz: i32) {
        self.modified.insert((cx, cz));
    }

    pub fn wants_save(&self) -> bool {
        self.last_save.elapsed() > SAVE_INTERVAL
    }

    /// Writes modified chunks and `level.dat` (player position, time).
    pub fn save(&mut self, world: &World, player: &Player, day_time: i64, ticks: i64) -> anyhow::Result<()> {
        let dirty: Vec<&Chunk> = self.modified.iter().filter_map(|p| world.chunk(p.0, p.1)).collect();
        let count = dirty.len();
        anvil::save_chunks(&self.dir, dirty, &mut self.backed_up)?;
        self.modified.clear();
        self.info.day_time = day_time;
        self.info.time = ticks;
        self.info.player = Some(PlayerInfo {
            pos: player.pos.to_array(),
            yaw: player.yaw,
            pitch: player.pitch,
            flying: player.flying,
        });
        anvil::write_level(&self.dir, &self.info, self.level_root.as_ref())?;
        self.last_save = Instant::now();
        tracing::info!("saved {} ({count} chunks)", self.dir.display());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_creative_worlds_and_refuses_survival_ones() {
        let saves = tempfile::tempdir().unwrap();
        let world = LocalWorld::open(saves.path(), "New", Some(7)).unwrap();
        assert_eq!((world.info.game_type, world.info.seed), (1, 7));

        let mut info = world.info.clone();
        info.game_type = 0;
        let dir = saves.path().join("Survival");
        std::fs::create_dir_all(&dir).unwrap();
        anvil::write_level(&dir, &info, None).unwrap();
        let err = LocalWorld::open(saves.path(), "Survival", None).err().unwrap();
        assert!(err.to_string().contains("survival world"), "{err}");
    }
}
