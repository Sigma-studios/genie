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
/// After the desktop's theme and the ones it inherits from.
const FALLBACKS: [&str; 4] = ["breeze", "hicolor", "Adwaita", "gnome"];

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
                    Icon::Theme(name) => match embedded(name) {
                        Some(svg) => render_svg(svg),
                        None => index.find(name).and_then(|path| load_theme_icon(&path)),
                    },
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

/// Logos no icon theme has, built in.
fn embedded(name: &str) -> Option<&'static [u8]> {
    let svg: &[u8] = match name {
        "genie-amazon" => include_bytes!("../assets/icons/amazon.svg"),
        "genie-ebay" => include_bytes!("../assets/icons/ebay.svg"),
        "genie-ecosia" => include_bytes!("../assets/icons/ecosia.svg"),
        "genie-github" => include_bytes!("../assets/icons/github.svg"),
        "genie-googlemaps" => include_bytes!("../assets/icons/googlemaps.svg"),
        "genie-googlescholar" => include_bytes!("../assets/icons/googlescholar.svg"),
        "genie-googletranslate" => include_bytes!("../assets/icons/googletranslate.svg"),
        "genie-spotify" => include_bytes!("../assets/icons/spotify.svg"),
        "genie-wolfram" => include_bytes!("../assets/icons/wolfram.svg"),
        "genie-youtube" => include_bytes!("../assets/icons/youtube.svg"),
        _ => return None,
    };
    Some(svg)
}

/// Icon names to files, across the themes and pixmaps. Walked once.
struct ThemeIndex(HashMap<String, PathBuf>);

impl ThemeIndex {
    fn build() -> Self {
        let others = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
        let dirs: Vec<PathBuf> = std::iter::once(config::data_home())
            .chain(others.split(':').filter(|d| !d.is_empty()).map(PathBuf::from))
            .collect();
        let themes = themes(&dirs);
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

    /// The full-colour icon, else its symbolic one: COSMIC's themes have
    /// many actions (media, save, emotes) only as symbolic.
    fn find(&self, name: &str) -> Option<PathBuf> {
        self.0.get(name).or_else(|| self.0.get(&format!("{name}-symbolic"))).cloned()
    }
}

/// The desktop's theme, then what it inherits (its `index.theme`, depth
/// first, as the icon theme spec walks it), then the fallbacks.
fn themes(dirs: &[PathBuf]) -> Vec<String> {
    themes_from(theme_setting(), dirs)
}

fn themes_from(theme: String, dirs: &[PathBuf]) -> Vec<String> {
    let mut themes = Vec::new();
    let mut stack = vec![theme];
    while let Some(theme) = stack.pop() {
        if themes.contains(&theme) {
            continue;
        }
        let parents = dirs.iter().find_map(|dir| inherits(&dir.join("icons").join(&theme).join("index.theme")));
        themes.push(theme);
        stack.extend(parents.unwrap_or_default().into_iter().rev());
    }
    for theme in FALLBACKS {
        if !themes.iter().any(|t| t == theme) {
            themes.push(theme.into());
        }
    }
    themes
}

fn inherits(index: &Path) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(index).ok()?;
    let line = text.lines().find_map(|line| line.strip_prefix("Inherits="))?;
    Some(line.split(',').map(str::trim).filter(|t| !t.is_empty()).map(String::from).collect())
}

/// COSMIC's theme (its toolkit's `icon_theme`, "Cosmic" unless changed),
/// else Plasma's from kdeglobals, else Breeze dark.
fn theme_setting() -> String {
    let cosmic = config::config_home().join("cosmic/com.system76.CosmicTk/v1/icon_theme");
    if let Ok(text) = std::fs::read_to_string(cosmic) {
        let theme = text.trim().trim_matches('"');
        if !theme.is_empty() {
            return theme.into();
        }
    }
    if std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.split(':').any(|d| d.eq_ignore_ascii_case("COSMIC"))) {
        return "Cosmic".into();
    }
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

/// A symbolic icon is a dark shape meant to be recoloured: drawn in the
/// text colour, so it shows on the dark window.
fn load_theme_icon(path: &Path) -> Option<ColorImage> {
    let mut image = load_file(path)?;
    if path.file_stem().and_then(|s| s.to_str()).is_some_and(|s| s.ends_with("-symbolic")) {
        tint(&mut image, crate::launcher::TEXT);
    }
    Some(image)
}

fn tint(image: &mut ColorImage, color: egui::Color32) {
    for pixel in &mut image.pixels {
        *pixel = egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), pixel.a());
    }
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
    fn every_engine_has_a_logo_that_renders() {
        for engine in crate::config::Config::default().websearch {
            let name = engine.icon.expect("an icon");
            let svg = embedded(&name).unwrap_or_else(|| panic!("no logo for {name}"));
            let image = render_svg(svg).unwrap_or_else(|| panic!("{name} doesn't render"));
            assert!(image.pixels.iter().any(|pixel| pixel.a() > 0), "{name} is blank");
        }
    }

    #[test]
    fn symbolic_icons_stand_in_and_take_the_text_colour() {
        let mut found = HashMap::new();
        add(&mut found, 0, Path::new("/i/scalable/actions/media-record-symbolic.svg"));
        add(&mut found, 0, Path::new("/i/scalable/actions/system-shutdown.svg"));
        add(&mut found, 0, Path::new("/i/scalable/actions/system-shutdown-symbolic.svg"));
        let index = ThemeIndex(found.into_iter().map(|(name, (_, _, path))| (name, path)).collect());
        assert_eq!(index.find("media-record").unwrap(), Path::new("/i/scalable/actions/media-record-symbolic.svg"));
        assert_eq!(index.find("system-shutdown").unwrap(), Path::new("/i/scalable/actions/system-shutdown.svg"));

        let mut image = ColorImage::new([2, 1], vec![egui::Color32::from_black_alpha(255), egui::Color32::TRANSPARENT]);
        tint(&mut image, crate::launcher::TEXT);
        assert_eq!(image.pixels[0], crate::launcher::TEXT);
        assert_eq!(image.pixels[1].a(), 0);
    }

    #[test]
    fn a_theme_brings_what_it_inherits_before_the_fallbacks() {
        let root = std::env::temp_dir().join(format!("genie-themes-{}", std::process::id()));
        for (theme, parents) in [("Cosmic", "Pop,hicolor"), ("Pop", "Adwaita")] {
            let dir = root.join("icons").join(theme);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("index.theme"), format!("[Icon Theme]\nInherits={parents}\n")).unwrap();
        }
        let themes = themes_from("Cosmic".into(), &[root.clone()]);
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(themes[..4], ["Cosmic", "Pop", "Adwaita", "hicolor"]);
        assert!(themes.iter().any(|t| t == "breeze"));
    }

    #[test]
    fn sizes_come_from_folders() {
        assert_eq!(size_in_path(Path::new("/i/64x64/a.png")), 64);
        assert_eq!(size_in_path(Path::new("/i/apps/48/a.png")), 48);
        assert_eq!(size_in_path(Path::new("/i/a.png")), 1);
    }
}
