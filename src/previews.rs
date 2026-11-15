//! File previews, made by Fractale's previewers (the `vignette` crate) and
//! kept in the shared thumbnail cache, so what Fractale or Dolphin already
//! made shows at once.
//!
//! The rows and the card beside them ask while they're drawn; at the end of
//! the frame the requests replace the last frame's, so arrowing past a
//! hundred files doesn't queue a hundred decodes. The workers start on the
//! first request, not with Génie. Finished previews become textures within a
//! per-frame upload budget, and the least recently drawn are dropped above a
//! memory cap: Génie stays running, so this is what it keeps between shows.
//!
//! Videos, sounds and Orchestre songs also play in the card, as they do in
//! Fractale when the pointer rests on them: here, once the card has shown
//! the same file for half a second, however it got there.

use crate::Event;
use crate::plugin::Preview;
use egui::TextureHandle;
use egui::TextureOptions;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::Duration;
use std::time::Instant;
use vignette::Key;
use vignette::Ready;
use vignette::Rendered;
use vignette::Request;
use vignette::Thumbs;
use vignette::cache::Bucket;
use vignette::playback::Kind;
use vignette::playback::Playback;

/// Smooth when shrinking, sharp pixels when enlarging (no blur on small
/// pictures and pixel art), as in Fractale.
const OPTIONS: TextureOptions = TextureOptions {
    magnification: egui::TextureFilter::Nearest,
    minification: egui::TextureFilter::Linear,
    wrap_mode: egui::TextureWrapMode::ClampToEdge,
    mipmap_mode: None,
};
/// Texture memory kept between shows.
const MEMORY_CAP: usize = 64 << 20;
/// Pixels uploaded per frame at most, so a list of arriving previews
/// doesn't stall a frame.
const UPLOAD_BUDGET: usize = 2 << 20;
/// How long the card shows a file before it plays: long enough that
/// arrowing past a video doesn't start it.
const PLAY_DELAY: Duration = Duration::from_millis(500);

enum Slot {
    /// Asked for, not back yet.
    Pending,
    /// Back, waiting for its turn to upload.
    Arrived,
    /// No preview possible: the type icon stays.
    Failed,
    Ready(Texture),
}

struct Texture {
    /// One frame, or an animation's frames with their durations.
    frames: Vec<(TextureHandle, Duration)>,
    size: egui::Vec2,
    bytes: usize,
    used: u64,
}

/// What to draw.
pub struct Shown {
    pub texture: egui::TextureId,
    /// In pixels.
    pub size: egui::Vec2,
}

pub struct Previews {
    events: Sender<Event>,
    pub player: Player,
    /// Started on the first request.
    thumbs: Option<Thumbs>,
    slots: HashMap<Key, Slot>,
    arrived: VecDeque<(Key, Rendered)>,
    /// This frame's requests, most wanted first.
    wanted: Vec<Request>,
    sent: Vec<Key>,
    frame: u64,
    memory: usize,
    /// Animations play from here.
    epoch: Instant,
}

impl Previews {
    pub fn new(events: Sender<Event>) -> Self {
        Self {
            player: Player::new(events.clone()),
            events,
            thumbs: None,
            slots: HashMap::new(),
            arrived: VecDeque::new(),
            wanted: Vec::new(),
            sent: Vec::new(),
            frame: 0,
            memory: 0,
            epoch: Instant::now(),
        }
    }

    /// The preview of `preview` for `pixels` physical pixels: that size, or
    /// meanwhile another size of the same file (the card's large one makes
    /// a fine row icon). Asks for it if it isn't there. Animations ask for
    /// the next frame.
    pub fn get(&mut self, ctx: &egui::Context, preview: &Preview, pixels: f32) -> Option<Shown> {
        let key = key(preview, pixels);
        self.want(&key, preview.size);
        let found = std::iter::once(key.bucket)
            .chain([Bucket::Normal, Bucket::Large, Bucket::XLarge, Bucket::XXLarge])
            .map(|bucket| Key { bucket, ..key.clone() })
            .find(|k| matches!(self.slots.get(k), Some(Slot::Ready(_))))?;
        let frame = self.frame;
        let Some(Slot::Ready(texture)) = self.slots.get_mut(&found) else { return None };
        texture.used = frame;
        let (index, next) = texture.frame_at(self.epoch.elapsed());
        if let Some(next) = next {
            ctx.request_repaint_after(next);
        }
        Some(Shown { texture: texture.frames[index].0.id(), size: texture.size })
    }

    fn want(&mut self, key: &Key, size: u64) {
        if matches!(self.slots.get(key), None | Some(Slot::Pending)) && !self.wanted.iter().any(|r| r.key == *key) {
            self.wanted.push(Request { key: key.clone(), size });
        }
    }

    /// After drawing: take what's back, upload some, send what's wanted.
    pub fn end_frame(&mut self, ctx: &egui::Context) {
        if let Some(thumbs) = &self.thumbs {
            while let Ok(ready) = thumbs.rx.try_recv() {
                match ready {
                    Ready::Done(key, rendered) => {
                        self.slots.insert(key.clone(), Slot::Arrived);
                        self.arrived.push_back((key, rendered));
                    }
                    Ready::Failed(key) => {
                        self.slots.insert(key, Slot::Failed);
                    }
                }
            }
        }
        let mut budget = UPLOAD_BUDGET;
        while budget > 0 {
            let Some((key, rendered)) = self.arrived.pop_front() else { break };
            let texture = upload(ctx, &key, rendered, self.frame);
            budget = budget.saturating_sub(texture.bytes / 4);
            self.memory += texture.bytes;
            if let Some(Slot::Ready(old)) = self.slots.insert(key, Slot::Ready(texture)) {
                self.memory -= old.bytes;
            }
        }
        if !self.arrived.is_empty() {
            ctx.request_repaint();
        }

        let wanted = std::mem::take(&mut self.wanted);
        let keys: Vec<Key> = wanted.iter().map(|r| r.key.clone()).collect();
        if keys != self.sent {
            // What's no longer wanted is forgotten, to be asked for again later.
            let set: HashSet<&Key> = keys.iter().collect();
            self.slots.retain(|k, s| !matches!(s, Slot::Pending) || set.contains(k));
            for k in &keys {
                self.slots.entry(k.clone()).or_insert(Slot::Pending);
            }
            if !wanted.is_empty() || self.thumbs.is_some() {
                let events = self.events.clone();
                let thumbs = self.thumbs.get_or_insert_with(|| {
                    Thumbs::spawn(Arc::new(move || {
                        let _ = events.send(Event::Wake);
                    }))
                });
                thumbs.want(wanted);
            }
            self.sent = keys;
        }
        self.evict();
        self.frame += 1;
    }

    /// The least recently drawn go above the cap (never this frame's).
    fn evict(&mut self) {
        if self.memory <= MEMORY_CAP {
            return;
        }
        let mut ready: Vec<(u64, Key, usize)> = self
            .slots
            .iter()
            .filter_map(|(k, s)| match s {
                Slot::Ready(t) if t.used < self.frame => Some((t.used, k.clone(), t.bytes)),
                _ => None,
            })
            .collect();
        ready.sort_by_key(|r| r.0);
        for (_, k, bytes) in ready {
            if self.memory <= MEMORY_CAP * 3 / 4 {
                break;
            }
            self.slots.remove(&k);
            self.memory -= bytes;
        }
    }
}

/// Plays the card's file, if it moves.
pub struct Player {
    events: Sender<Event>,
    /// The card's file, and since when.
    subject: Option<(PathBuf, Instant)>,
    playing: Option<Playback>,
    texture: Option<TextureHandle>,
}

impl Player {
    fn new(events: Sender<Event>) -> Self {
        Self { events, subject: None, playing: None, texture: None }
    }

    /// Every frame, with the card's file (`None` without a card). A new file
    /// stops what played; the same one for long enough starts. `pixels` is
    /// how large a video's frames need to be.
    pub fn follow(&mut self, ctx: &egui::Context, path: Option<&Path>, pixels: u32) {
        if self.subject.as_ref().map(|(p, _)| p.as_path()) != path {
            self.stop();
            self.subject = path.map(|p| (p.to_path_buf(), Instant::now()));
        }
        let Some((path, since)) = &self.subject else { return };
        if self.playing.is_none() {
            let Some(kind) = Kind::of(path) else { return };
            let waited = since.elapsed();
            if waited < PLAY_DELAY {
                ctx.request_repaint_after(PLAY_DELAY - waited);
                return;
            }
            let events = self.events.clone();
            let wake = Arc::new(move || {
                let _ = events.send(Event::Wake);
            });
            self.playing = Some(Playback::start(path.clone(), kind, pixels, wake));
        }
        // Videos wake Génie with each frame; sounds don't, and their bar moves.
        if self.playing.as_ref().is_some_and(|p| p.kind != Kind::Video) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// The video's newest frame, in pixels, once there is one: until then
    /// the still stays.
    pub fn video(&mut self, ctx: &egui::Context) -> Option<Shown> {
        let playing = self.playing.as_ref().filter(|p| p.kind == Kind::Video)?;
        if let Some(frame) = playing.frame() {
            let size = [frame.width as usize, frame.height as usize];
            let image = egui::ColorImage::from_rgba_unmultiplied(size, &frame.rgba);
            match &mut self.texture {
                Some(texture) if texture.size() == size => texture.set(image, OPTIONS),
                _ => self.texture = Some(ctx.load_texture("playing", image, OPTIONS)),
            }
        }
        let texture = self.texture.as_ref()?;
        Some(Shown { texture: texture.id(), size: texture.size_vec2() })
    }

    /// How far a sound is, 0 to 1, while one plays.
    pub fn progress(&self) -> Option<f32> {
        self.playing.as_ref().filter(|p| p.kind != Kind::Video)?.progress()
    }

    /// Silence, when the card goes or Génie hides.
    pub fn stop(&mut self) {
        self.subject = None;
        self.playing = None;
        self.texture = None;
    }
}

fn key(preview: &Preview, pixels: f32) -> Key {
    Key { path: preview.path.clone(), mtime: preview.mtime, bucket: Bucket::for_pixels(pixels) }
}

impl Texture {
    /// Which frame shows now, and how long until the next.
    fn frame_at(&self, t: Duration) -> (usize, Option<Duration>) {
        if self.frames.len() < 2 {
            return (0, None);
        }
        let total: Duration = self.frames.iter().map(|f| f.1).sum();
        let mut at = Duration::from_nanos((t.as_nanos() % total.as_nanos().max(1)) as u64);
        for (i, (_, d)) in self.frames.iter().enumerate() {
            if at < *d {
                return (i, Some(*d - at));
            }
            at -= *d;
        }
        (0, Some(self.frames[0].1))
    }
}

fn upload(ctx: &egui::Context, key: &Key, rendered: Rendered, frame: u64) -> Texture {
    let name = key.path.to_string_lossy();
    let frames: Vec<(image::RgbaImage, Duration)> = match rendered {
        Rendered::Still(i) => vec![(i, Duration::ZERO)],
        Rendered::Animated(f) => f,
    };
    let size = egui::vec2(frames[0].0.width() as f32, frames[0].0.height() as f32);
    let mut bytes = 0;
    let frames = frames
        .into_iter()
        .map(|(image, duration)| {
            bytes += image.len();
            let color = egui::ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], &image);
            (ctx.load_texture(name.as_ref(), color, OPTIONS), duration)
        })
        .collect();
    Texture { frames, size, bytes, used: frame }
}

/// `size` fitted in `rect`, centred, never larger than its pixels.
pub fn fit(rect: egui::Rect, size: egui::Vec2, pixels_per_point: f32) -> egui::Rect {
    let natural = size / pixels_per_point;
    let scale = (rect.width() / natural.x).min(rect.height() / natural.y).min(1.0);
    egui::Rect::from_center_size(rect.center(), natural * scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_inside_and_keeps_the_shape() {
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(100.0, 100.0));
        let wide = fit(rect, egui::vec2(400.0, 200.0), 2.0);
        assert_eq!(wide.size(), egui::vec2(100.0, 50.0));
        assert_eq!(wide.center(), rect.center());
        let small = fit(rect, egui::vec2(40.0, 20.0), 2.0);
        assert_eq!(small.size(), egui::vec2(20.0, 10.0), "not enlarged");
    }

    #[test]
    fn plays_only_once_the_card_stays_on_a_file() {
        let (events, _) = std::sync::mpsc::channel();
        let mut player = Player::new(events);
        let ctx = egui::Context::default();
        let song = Path::new("/nowhere/a.mp3");
        player.follow(&ctx, Some(song), 100);
        let since = player.subject.as_ref().unwrap().1;
        player.follow(&ctx, Some(song), 100);
        assert!(player.playing.is_none(), "not before half a second");
        assert_eq!(player.subject.as_ref().unwrap().1, since, "the same file keeps its wait");
        player.follow(&ctx, Some(Path::new("/nowhere/b.mp3")), 100);
        assert_eq!(player.subject.as_ref().unwrap().0, Path::new("/nowhere/b.mp3"), "another file waits afresh");
        player.follow(&ctx, None, 0);
        assert!(player.subject.is_none() && player.playing.is_none());
        player.follow(&ctx, Some(Path::new("/nowhere/a.png")), 100);
        player.subject.as_mut().unwrap().1 -= PLAY_DELAY;
        player.follow(&ctx, Some(Path::new("/nowhere/a.png")), 100);
        assert!(player.playing.is_none(), "pictures don't play");
    }

    #[test]
    fn frames_follow_their_durations() {
        let ctx = egui::Context::default();
        let blank = || ctx.load_texture("t", egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]), OPTIONS);
        let ms = Duration::from_millis;
        let texture = Texture { frames: vec![(blank(), ms(100)), (blank(), ms(50))], size: egui::Vec2::splat(1.0), bytes: 8, used: 0 };
        assert_eq!(texture.frame_at(ms(30)), (0, Some(ms(70))));
        assert_eq!(texture.frame_at(ms(120)), (1, Some(ms(30))));
        assert_eq!(texture.frame_at(ms(160)), (0, Some(ms(90))));
    }
}
