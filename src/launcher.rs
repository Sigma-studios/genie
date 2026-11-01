//! The launcher itself: the input, the results, and Albert's layout, down to
//! its numbers. Its sizes were in points, so text here is 4/3 of those.

use crate::history::History;
use crate::icons::Icons;
use crate::plugin::Action;
use crate::plugin::Item;
use crate::plugin::Runner;
use egui::Color32;
use egui::CornerRadius;
use egui::FontId;
use egui::Key;
use egui::Modifiers;
use egui::Rect;
use egui::Sense;
use egui::Stroke;
use egui::Vec2;
use egui::text::CCursor;
use egui::text::CCursorRange;
use egui::text::LayoutJob;
use egui::text::TextWrapping;

pub const WIDTH: f32 = 640.0;
pub const SHADOW: f32 = 80.0;
const PADDING: f32 = 7.0;
const SPACING: f32 = 7.0;
const INPUT_FONT: f32 = 19.0 * 4.0 / 3.0;
const INPUT_PADDING: f32 = 5.0;
const INPUT_HEIGHT: f32 = 32.0 + INPUT_PADDING * 2.0;
const VISIBLE_ROWS: usize = 5;
const ICON: f32 = 39.0;
const ROW_PADDING: f32 = 6.0;
const ROW_HEIGHT: f32 = ICON + ROW_PADDING * 2.0;
const ROW_SPACING: f32 = 2.0;
const ICON_GAP: f32 = 6.0;
const TITLE_FONT: f32 = 14.0 * 4.0 / 3.0;
const SUBTITLE_FONT: f32 = 9.0 * 4.0 / 3.0;
const ACTION_HEIGHT: f32 = 30.0;

const WINDOW: Color32 = Color32::from_rgb(0x20, 0x23, 0x26);
const BORDER: Color32 = Color32::from_rgb(0x43, 0x48, 0x4d);
const INPUT: Color32 = Color32::from_rgb(0x14, 0x16, 0x18);
const SELECTION: Color32 = Color32::from_rgb(0x1f, 0x4e, 0x68);
const TEXT: Color32 = Color32::from_rgb(0xfc, 0xfc, 0xfc);
const SUBTEXT: Color32 = Color32::from_rgb(0xa1, 0xa9, 0xb1);

pub fn surface_size() -> (u32, u32) {
    let rows = VISIBLE_ROWS as f32 * ROW_HEIGHT + (VISIBLE_ROWS - 1) as f32 * ROW_SPACING;
    let panel = PADDING + INPUT_HEIGHT + SPACING + rows + PADDING;
    ((WIDTH + SHADOW * 2.0) as u32, (panel + SHADOW * 2.0) as u32)
}

pub enum Request {
    Hide,
    Run(Action, Item),
}

pub struct Launcher {
    input: String,
    /// The query the results are for.
    query: String,
    items: Vec<Item>,
    selected: usize,
    action: usize,
    /// Results come for this query.
    generation: u64,
    /// Plugins that haven't finished.
    pending: usize,
    /// The list is still the previous query's: it stays until the first new
    /// results, so that typing doesn't flicker.
    stale: bool,
    scroll_to_selected: bool,
    cursor_to_end: bool,
    /// Up in the input walks past queries. Where we are, if we're doing that.
    recall: Option<usize>,
    pub runner: Runner,
    pub history: History,
}

impl Launcher {
    pub fn new(runner: Runner, history: History) -> Self {
        Self {
            input: String::new(),
            query: String::new(),
            items: Vec::new(),
            selected: 0,
            action: 0,
            generation: 0,
            pending: 0,
            stale: false,
            scroll_to_selected: false,
            cursor_to_end: false,
            recall: None,
            runner,
            history,
        }
    }

    pub fn history_query(&self) -> String {
        self.query.clone()
    }

    pub fn reset(&mut self) {
        self.set_input(String::new());
        self.recall = None;
    }

    pub fn set_input(&mut self, text: String) {
        self.input = text;
        self.cursor_to_end = true;
        self.query_changed();
    }

    fn query_changed(&mut self) {
        self.query = self.input.clone();
        let (generation, pending) = self.runner.run(&self.query);
        self.generation = generation;
        self.pending = pending;
        self.stale = true;
        if pending == 0 {
            self.items.clear();
            self.selected = 0;
            self.stale = false;
        }
    }

    pub fn results(&mut self, generation: u64, mut items: Vec<Item>) {
        if generation != self.generation {
            return;
        }
        for item in &mut items {
            item.score += self.history.boost(item.plugin, &item.id, &self.query);
        }
        let selected = (!self.stale).then(|| self.items.get(self.selected).map(|i| (i.plugin, i.id.clone()))).flatten();
        if self.stale {
            self.items.clear();
            self.stale = false;
            self.selected = 0;
            self.scroll_to_selected = true;
        }
        self.items.extend(items);
        sort(&mut self.items);
        if let Some((plugin, id)) = selected {
            self.selected = self.items.iter().position(|i| i.plugin == plugin && i.id == id).unwrap_or(0);
        }
    }

    pub fn finished(&mut self, generation: u64) {
        if generation != self.generation {
            return;
        }
        self.pending = self.pending.saturating_sub(1);
        if self.pending != 0 {
            return;
        }
        if self.stale {
            self.items.clear();
            self.stale = false;
        }
        // Files in the background match almost anything, so they don't count.
        if self.items.iter().all(|item| item.background) {
            self.items.extend(self.runner.fallbacks(&self.query));
            sort(&mut self.items);
            self.select(0);
        }
    }

    fn select(&mut self, index: usize) {
        self.selected = index;
        self.action = 0;
        self.scroll_to_selected = true;
    }

    fn step(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let last = self.items.len() as isize - 1;
        self.select((self.selected as isize + delta).clamp(0, last) as usize);
    }

    /// One step further back or forward through past queries.
    fn recall(&mut self, older: bool) {
        let next = match (self.recall, older) {
            (None, true) => 0,
            (None, false) => return,
            (Some(at), true) => at + 1,
            (Some(0), false) => {
                self.recall = None;
                self.set_input(String::new());
                return;
            }
            (Some(at), false) => at - 1,
        };
        if let Some(text) = self.history.query(next) {
            let text = text.to_string();
            self.recall = Some(next);
            self.set_input(text);
        }
    }

    fn activate(&mut self, action: usize, requests: &mut Vec<Request>) {
        let Some(item) = self.items.get(self.selected) else { return };
        if let Some(run) = item.actions.get(action) {
            requests.push(Request::Run(run.clone(), item.clone()));
        } else if let Some(completion) = &item.completion {
            let completion = completion.clone();
            self.set_input(completion);
        }
    }

    pub fn activate_selected(&mut self) -> Vec<Request> {
        let mut requests = Vec::new();
        self.activate(0, &mut requests);
        requests
    }

    fn keys(&mut self, ctx: &egui::Context, requests: &mut Vec<Request>) {
        let pressed = |modifiers, key| ctx.input_mut(|i| i.consume_key(modifiers, key));
        if pressed(Modifiers::NONE, Key::Escape) {
            requests.push(Request::Hide);
        }
        // Alt lists the selected item's actions, and these move through them.
        let actions = self.items.get(self.selected).map_or(0, |i| i.actions.len());
        if pressed(Modifiers::ALT, Key::ArrowDown) {
            self.action = (self.action + 1).min(actions.saturating_sub(1));
        }
        if pressed(Modifiers::ALT, Key::ArrowUp) {
            self.action = self.action.saturating_sub(1);
        }
        if pressed(Modifiers::ALT, Key::Enter) {
            let action = self.action;
            self.activate(action, requests);
        }
        if pressed(Modifiers::NONE, Key::Enter) {
            self.activate(0, requests);
        }
        if pressed(Modifiers::NONE, Key::ArrowDown) {
            if self.recall.is_some() && self.selected == 0 {
                self.recall(false);
            } else {
                self.step(1);
            }
        }
        if pressed(Modifiers::NONE, Key::ArrowUp) {
            if self.selected == 0 {
                self.recall(true);
            } else {
                self.step(-1);
            }
        }
        if pressed(Modifiers::NONE, Key::PageDown) {
            self.step(VISIBLE_ROWS as isize);
        }
        if pressed(Modifiers::NONE, Key::PageUp) {
            self.step(-(VISIBLE_ROWS as isize));
        }
        if pressed(Modifiers::NONE, Key::Tab)
            && let Some(completion) = self.items.get(self.selected).and_then(|i| i.completion.clone())
        {
            self.set_input(completion);
        }
    }

    /// Draws a frame. Returns what to do, and where the panel is: everything
    /// around it, the shadow included, lets clicks through.
    pub fn draw(&mut self, ui: &mut egui::Ui, icons: &mut Icons) -> (Vec<Request>, Rect) {
        let ctx = ui.ctx().clone();
        let mut requests = Vec::new();
        self.keys(&ctx, &mut requests);
        let alt = ctx.input(|i| i.modifiers.alt);

        let frame = egui::Frame::new()
            .fill(WINDOW)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(CornerRadius::same(18))
            .inner_margin(PADDING)
            .outer_margin(egui::Margin { left: SHADOW as i8, right: SHADOW as i8, top: SHADOW as i8, bottom: 0 })
            .shadow(egui::Shadow { offset: [0, 8], blur: 60, spread: 0, color: Color32::from_black_alpha(130) });

        let panel = frame.show(ui, |ui| {
            ui.set_width(WIDTH - PADDING * 2.0 - 2.0);
            ui.spacing_mut().item_spacing = Vec2::new(0.0, SPACING);
            self.draw_input(ui);
            if !self.items.is_empty() {
                if alt {
                    self.draw_actions(ui, &mut requests);
                } else {
                    self.draw_rows(ui, icons, &mut requests);
                }
            }
        });
        (requests, panel.response.rect)
    }

    fn draw_input(&mut self, ui: &mut egui::Ui) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), INPUT_HEIGHT), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(11), INPUT);
        let inner = rect.shrink2(Vec2::new(INPUT_PADDING + 4.0, INPUT_PADDING));
        let edit = egui::TextEdit::singleline(&mut self.input)
            .id_salt("input")
            .font(FontId::proportional(INPUT_FONT))
            .text_color(TEXT)
            .frame(egui::Frame::NONE)
            .desired_width(inner.width())
            .margin(Vec2::ZERO)
            .lock_focus(true);
        let response = ui.put(inner, edit);
        response.request_focus();
        if response.changed() {
            self.recall = None;
            self.query_changed();
        }
        if std::mem::take(&mut self.cursor_to_end)
            && let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), response.id)
        {
            let end = CCursor::new(self.input.chars().count());
            state.cursor.set_char_range(Some(CCursorRange::one(end)));
            state.store(ui.ctx(), response.id);
        }
    }

    fn draw_rows(&mut self, ui: &mut egui::Ui, icons: &mut Icons, requests: &mut Vec<Request>) {
        let height = VISIBLE_ROWS as f32 * ROW_HEIGHT + (VISIBLE_ROWS - 1) as f32 * ROW_SPACING;
        let scroll_to = std::mem::take(&mut self.scroll_to_selected);
        let mut clicked = None;
        egui::ScrollArea::vertical()
            .max_height(height)
            .auto_shrink([false, true])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(0.0, ROW_SPACING);
                for (index, item) in self.items.iter().enumerate() {
                    let selected = index == self.selected;
                    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::click());
                    if !ui.is_rect_visible(rect) {
                        continue;
                    }
                    if selected {
                        ui.painter().rect_filled(rect, CornerRadius::same(11), SELECTION);
                        if scroll_to {
                            ui.scroll_to_rect(rect, None);
                        }
                    } else if response.hovered() {
                        ui.painter().rect_filled(rect, CornerRadius::same(11), Color32::from_white_alpha(10));
                    }
                    let icon_rect = Rect::from_min_size(rect.min + Vec2::splat(ROW_PADDING), Vec2::splat(ICON));
                    if let Some(texture) = icons.get(&item.icon) {
                        let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                        ui.painter().image(texture, icon_rect, uv, Color32::WHITE);
                    }
                    let left = icon_rect.right() + ICON_GAP;
                    let width = rect.right() - ROW_PADDING - left;
                    let title = galley(ui, &item.title, TITLE_FONT, TEXT, width);
                    if item.subtitle.is_empty() {
                        ui.painter().galley(egui::pos2(left, rect.center().y - title.size().y / 2.0), title, TEXT);
                    } else {
                        let subtitle = galley(ui, &item.subtitle, SUBTITLE_FONT, SUBTEXT, width);
                        let total = title.size().y + subtitle.size().y;
                        let top = rect.center().y - total / 2.0;
                        let below = top + title.size().y;
                        ui.painter().galley(egui::pos2(left, top), title, TEXT);
                        ui.painter().galley(egui::pos2(left, below), subtitle, SUBTEXT);
                    }
                    if response.clicked() {
                        clicked = Some(index);
                    }
                }
            });
        if let Some(index) = clicked {
            self.select(index);
            self.activate(0, requests);
        }
    }

    fn draw_actions(&mut self, ui: &mut egui::Ui, requests: &mut Vec<Request>) {
        let Some(item) = self.items.get(self.selected) else { return };
        ui.spacing_mut().item_spacing = Vec2::new(0.0, ROW_SPACING);
        let mut clicked = None;
        for (index, action) in item.actions.iter().enumerate() {
            let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ACTION_HEIGHT), Sense::click());
            if index == self.action {
                ui.painter().rect_filled(rect, CornerRadius::same(11), SELECTION);
            }
            let text = galley(ui, &action.label, TITLE_FONT, TEXT, rect.width() - ROW_PADDING * 2.0);
            let at = egui::pos2(rect.left() + ROW_PADDING, rect.center().y - text.size().y / 2.0);
            ui.painter().galley(at, text, TEXT);
            if response.clicked() {
                clicked = Some(index);
            }
        }
        if let Some(index) = clicked {
            self.activate(index, requests);
        }
    }
}

/// Best first, background items last. Stable, so fallbacks keep their order.
fn sort(items: &mut [Item]) {
    items.sort_by(|a, b| a.background.cmp(&b.background).then(b.score.total_cmp(&a.score)));
}

/// One line, cut with an ellipsis when too long.
fn galley(ui: &egui::Ui, text: &str, size: f32, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_string(), FontId::proportional(size), color);
    job.wrap = TextWrapping::truncate_at_width(width);
    ui.painter().layout_job(job)
}

/// The desktop's font, as text everywhere else looks.
pub fn fonts(ctx: &egui::Context) {
    let output = std::process::Command::new("fc-match").args(["-f", "%{file}", "sans-serif"]).output();
    let Some(bytes) = output.ok().and_then(|o| std::fs::read(String::from_utf8_lossy(&o.stdout).as_ref()).ok()) else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("system".into(), egui::FontData::from_owned(bytes).into());
    fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "system".into());
    ctx.set_fonts(fonts);
}
