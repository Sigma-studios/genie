//! Icons, loaded off the UI thread: the UI asks, draws a blank until the event
//! comes, and keeps the texture for good.

use crate::Event;
use crate::config;
use crate::plugin::Icon;
use egui::ColorImage;
use egui::TextureHandle;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::mpsc::Sender;

/// Pixels, for 39 points at scale 2.
const SIZE: u32 = 78;
const THEMES: [&str; 4] = ["breeze", "hicolor", "Adwaita", "gnome"];

enum Slot {
    Loading,
    Missing,
    Ready(TextureHandle),
}

/// Textures belong to one egui context, so each window has its own Icons.
#[derive(Clone, Copy)]
pub enum Window {
    Launcher,
    CommandList,
}

pub struct Icons {
    slots: HashMap<Icon, Slot>,
    requests: Sender<Icon>,
}

impl Icons {
    pub fn new(events: Sender<Event>, window: Window) -> Self {
        let (requests, incoming) = mpsc::channel::<Icon>();
        let sender = events;
        std::thread::spawn(move || {
            let index = ThemeIndex::build();
            for icon in incoming {
                // Art comes over the network, which shouldn't hold up the rest.
                if let Icon::Url(url) = &icon {
                    let (url, sender, icon) = (url.clone(), sender.clone(), icon.clone());
                    std::thread::spawn(move || {
                        let image = fetch_url(&url).and_then(|path| load_file(&path));
                        let _ = sender.send(Event::Icon { window, icon, image });
                    });
                    continue;
                }
                let image = match &icon {
                    Icon::Theme(name) => index.find(name).and_then(|path| load_file(&path)),
                    Icon::File(path) => load_file(path),
                    Icon::Url(_) => None,
                };
                let _ = sender.send(Event::Icon { window, icon, image });
            }
        });
        Self { slots: HashMap::new(), requests }
    }

    pub fn loaded(&mut self, ctx: &egui::Context, icon: Icon, image: Option<ColorImage>) {
        let slot = match image {
            Some(image) => Slot::Ready(ctx.load_texture(format!("{icon:?}"), image, egui::TextureOptions::LINEAR)),
            None => Slot::Missing,
        };
        self.slots.insert(icon, slot);
    }

    /// The texture, or the generic one meanwhile.
    pub fn get(&mut self, icon: &Icon) -> Option<egui::TextureId> {
        match self.slots.get(icon) {
            Some(Slot::Ready(texture)) => Some(texture.id()),
            Some(Slot::Missing) => {
                let generic = Icon::Theme(GENERIC.into());
                if *icon == generic {
                    return None;
                }
                self.get(&generic)
            }
            Some(Slot::Loading) => None,
            None => {
                self.slots.insert(icon.clone(), Slot::Loading);
                let _ = self.requests.send(icon.clone());
                None
            }
        }
    }
}

const GENERIC: &str = "application-x-executable";

/// Icon names to files, across the themes and pixmaps. Walked once.
struct ThemeIndex(HashMap<String, PathBuf>);

impl ThemeIndex {
    fn build() -> Self {
        let others = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
        let dirs: Vec<PathBuf> = std::iter::once(config::data_home())
            .chain(others.split(':').filter(|d| !d.is_empty()).map(PathBuf::from))
            .collect();
        let mut themes = vec![theme_setting()];
        themes.extend(THEMES.iter().map(|t| t.to_string()));
        let mut found: HashMap<String, (usize, u32, PathBuf)> = HashMap::new();
        for (rank, theme) in themes.iter().enumerate() {
            for dir in &dirs {
                walk(&dir.join("icons").join(theme), &mut |path| add(&mut found, rank, path));
            }
        }
        for dir in &dirs {
            walk(&dir.join("pixmaps"), &mut |path| add(&mut found, themes.len(), path));
        }
        Self(found.into_iter().map(|(name, (_, _, path))| (name, path)).collect())
    }

    fn find(&self, name: &str) -> Option<PathBuf> {
        self.0.get(name).cloned()
    }
}

/// Plasma's theme, from kdeglobals, dark by default like the desktop here.
fn theme_setting() -> String {
    let text = std::fs::read_to_string(config::config_home().join("kdeglobals")).unwrap_or_default();
    let mut in_icons = false;
    for line in text.lines() {
        if line.starts_with('[') {
            in_icons = line == "[Icons]";
        } else if in_icons && let Some(theme) = line.strip_prefix("Theme=") {
            return theme.to_string();
        }
    }
    "breeze-dark".into()
}

fn walk(dir: &Path, visit: &mut dyn FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        // Through symlinks: Nix profiles are made of them.
        if path.is_dir() {
            walk(&path, visit);
        } else {
            visit(&path);
        }
    }
}

/// The earliest theme wins, and within it scalable, then the largest.
fn add(found: &mut HashMap<String, (usize, u32, PathBuf)>, rank: usize, path: &Path) {
    let Some(extension) = path.extension().and_then(|e| e.to_str()) else { return };
    let Some(name) = path.file_stem().and_then(|n| n.to_str()) else { return };
    let quality = match extension {
        "svg" => 100_000,
        "png" => size_in_path(path),
        _ => return,
    };
    let better = found.get(name).is_none_or(|(r, q, _)| rank < *r || (rank == *r && quality > *q));
    if better {
        found.insert(name.to_string(), (rank, quality, path.to_path_buf()));
    }
}

/// From a folder like `48x48`, `48` or `48@2x`.
fn size_in_path(path: &Path) -> u32 {
    path.components()
        .filter_map(|c| c.as_os_str().to_str())
        .filter_map(|c| c.split(['x', '@']).next()?.parse().ok())
        .max()
        .unwrap_or(1)
}

fn load_file(path: &Path) -> Option<ColorImage> {
    let bytes = std::fs::read(path).ok()?;
    let svg = path.extension().is_some_and(|e| e == "svg");
    let image = if svg { render_svg(&bytes) } else { decode(&bytes) };
    if image.is_none() {
        log::debug!("couldn't read the icon {}", path.display());
    }
    image
}

fn render_svg(bytes: &[u8]) -> Option<ColorImage> {
    let tree = resvg::usvg::Tree::from_data(bytes, &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(SIZE, SIZE)?;
    let size = tree.size();
    let scale = (SIZE as f32 / size.width()).min(SIZE as f32 / size.height());
    let offset_x = (SIZE as f32 - size.width() * scale) / 2.0;
    let offset_y = (SIZE as f32 - size.height() * scale) / 2.0;
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale).post_translate(offset_x, offset_y);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Some(ColorImage::from_rgba_premultiplied([SIZE as usize, SIZE as usize], pixmap.data()))
}

fn decode(bytes: &[u8]) -> Option<ColorImage> {
    let image = image::load_from_memory(bytes).ok()?.into_rgba8();
    let image = if image.width() > SIZE * 2 {
        image::imageops::resize(&image, SIZE, SIZE, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    let size = [image.width() as usize, image.height() as usize];
    Some(ColorImage::from_rgba_unmultiplied(size, image.as_raw()))
}

/// Downloaded art stays in the cache, by URL.
fn fetch_url(url: &str) -> Option<PathBuf> {
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    let dir = config::cache_home().join("genie/art");
    let path = dir.join(format!("{:016x}", hasher.finish()));
    if path.exists() {
        return Some(path);
    }
    let bytes = ureq::get(url).call().ok()?.body_mut().read_to_vec().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(&path, bytes).ok()?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalable_beats_big_and_earlier_themes_win() {
        let mut found = HashMap::new();
        add(&mut found, 1, Path::new("/i/48x48/apps/a.png"));
        add(&mut found, 1, Path::new("/i/256x256/apps/a.png"));
        assert_eq!(found["a"].2, Path::new("/i/256x256/apps/a.png"));
        add(&mut found, 1, Path::new("/i/scalable/apps/a.svg"));
        assert_eq!(found["a"].2, Path::new("/i/scalable/apps/a.svg"));
        add(&mut found, 0, Path::new("/first/16x16/apps/a.png"));
        assert_eq!(found["a"].2, Path::new("/first/16x16/apps/a.png"));
        add(&mut found, 0, Path::new("/first/16x16/apps/b.txt"));
        assert!(!found.contains_key("b"));
    }

    #[test]
    fn sizes_come_from_folders() {
        assert_eq!(size_in_path(Path::new("/i/64x64/a.png")), 64);
        assert_eq!(size_in_path(Path::new("/i/apps/48/a.png")), 48);
        assert_eq!(size_in_path(Path::new("/i/a.png")), 1);
    }
}
