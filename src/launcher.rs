//! The launcher itself: the input, the results, and Albert's layout, down to
//! its numbers. Its sizes were in points, so text here is 4/3 of those.

use crate::history::History;
use crate::icons::Icons;
use crate::plugin::Action;
use crate::plugin::Item;
use crate::plugin::LAST_TIER;
use crate::plugin::Runner;
use crate::previews;
use crate::previews::Previews;
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
/// The card beside the list with the file's preview, like Spotlight's.
const CARD_WIDTH: f32 = 270.0;
const CARD_GAP: f32 = 8.0;
/// The card's shadow is smaller than the panel's, to need less room.
const CARD_SHADOW: f32 = 40.0;
/// Room on each side of the panel: the card's on the right, and as much on
/// the left, so the panel stays in the middle of the screen whether or not
/// the card shows. 1276 points wide in all, under the 1280 of a laptop at
/// scale 2.
const SIDE: f32 = CARD_GAP + CARD_WIDTH + CARD_SHADOW;
const PADDING: f32 = 7.0;
const SPACING: f32 = 7.0;
const INPUT_FONT: f32 = 19.0 * 4.0 / 3.0;
const INPUT_PADDING: f32 = 5.0;
const INPUT_HEIGHT: f32 = 32.0 + INPUT_PADDING * 2.0;
const VISIBLE_ROWS: usize = 5;
/// Going down past the fifth row grows the list to this many, rather than
/// scrolling; past these, it scrolls.
const EXPANDED_ROWS: usize = 12;
/// The line under the list saying how many more there are.
const MORE_HEIGHT: f32 = 22.0;
const ICON: f32 = 39.0;
const ROW_PADDING: f32 = 6.0;
const ROW_HEIGHT: f32 = ICON + ROW_PADDING * 2.0;
const ROW_SPACING: f32 = 2.0;
const ICON_GAP: f32 = 6.0;
const TITLE_FONT: f32 = 14.0 * 4.0 / 3.0;
const SUBTITLE_FONT: f32 = 9.0 * 4.0 / 3.0;
const ACTION_HEIGHT: f32 = 30.0;

pub const WINDOW: Color32 = Color32::from_rgb(0x20, 0x23, 0x26);
const BORDER: Color32 = Color32::from_rgb(0x43, 0x48, 0x4d);
pub const INPUT: Color32 = Color32::from_rgb(0x14, 0x16, 0x18);
pub const SELECTION: Color32 = Color32::from_rgb(0x1f, 0x4e, 0x68);
pub const TEXT: Color32 = Color32::from_rgb(0xfc, 0xfc, 0xfc);
pub const SUBTEXT: Color32 = Color32::from_rgb(0xa1, 0xa9, 0xb1);

fn rows_height(rows: usize) -> f32 {
    rows as f32 * ROW_HEIGHT + rows.saturating_sub(1) as f32 * ROW_SPACING
}

/// The panel with five rows, which the card matches.
fn panel_height() -> f32 {
    PADDING + INPUT_HEIGHT + SPACING + rows_height(VISIBLE_ROWS) + PADDING
}

/// The panel grows downwards, so the surface has room for it at its
/// tallest: twelve rows and the line under them.
pub fn surface_size() -> (u32, u32) {
    let tallest = PADDING + INPUT_HEIGHT + SPACING + rows_height(EXPANDED_ROWS) + SPACING + MORE_HEIGHT + PADDING;
    ((WIDTH + SIDE * 2.0) as u32, (tallest + SHADOW * 2.0) as u32)
}

pub enum Request {
    Hide,
    Run(Action, Item),
}

/// An action waiting for a second Enter.
struct Confirming {
    question: String,
    action: Action,
    item: Item,
    /// Where the keyboard is. It starts on the action, so Enter twice runs it.
    on_action: bool,
}

pub struct Launcher {
    confirming: Option<Confirming>,
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
    /// The input's widget, to know where its cursor is.
    input_id: Option<egui::Id>,
    /// Alt+Up or Alt+Down opened the selected item's list of actions.
    actions_open: bool,
    /// Results held back until every plugin has finished.
    held: Vec<Item>,
    /// Enter came before the results: it runs as soon as they're in.
    enter_waiting: bool,
    /// The selection was moved by hand, so results still arriving don't reset it.
    selection_moved: bool,
    /// Up in the input walks past queries. Where we are, if we're doing that.
    recall: Option<usize>,
    /// The row the pointer moved onto: the card shows it rather than the
    /// selected one, until the keyboard moves the selection. A pointer that
    /// just sits where the window opens doesn't count.
    pointed: Option<usize>,
    /// Where the card was last frame, so moving onto it keeps it.
    card: Option<Rect>,
    /// Twelve rows rather than five, until the query changes.
    expanded: bool,
    /// How many are below the last row in sight.
    more: usize,
    pub runner: Runner,
    pub history: History,
}

impl Launcher {
    pub fn new(runner: Runner, history: History) -> Self {
        Self {
            confirming: None,
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
            input_id: None,
            actions_open: false,
            held: Vec::new(),
            enter_waiting: false,
            selection_moved: false,
            recall: None,
            pointed: None,
            card: None,
            expanded: false,
            more: 0,
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
        self.confirming = None;
        self.pointed = None;
        self.expanded = false;
        self.query = self.input.clone();
        let started = self.runner.run(&self.query);
        self.generation = started.generation;
        self.pending = started.pending;
        self.stale = true;
        self.held.clear();
        self.enter_waiting = false;
        self.selection_moved = false;
        if started.pending == 0 {
            self.items = started.items;
            sort(&mut self.items);
            self.select(0);
            self.selection_moved = false;
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
        let (held, items): (Vec<Item>, Vec<Item>) = items.into_iter().partition(|item| item.tier >= LAST_TIER);
        self.held.extend(held);
        if items.is_empty() {
            return;
        }
        self.publish(items);
    }

    fn publish(&mut self, items: Vec<Item>) {
        self.pointed = None;
        let selected = self.items.get(self.selected).filter(|_| self.selection_moved && !self.stale).map(|i| (i.plugin, i.id.clone()));
        if self.stale {
            self.items.clear();
            self.stale = false;
            self.selected = 0;
            self.scroll_to_selected = true;
        }
        self.items.extend(items);
        sort(&mut self.items);
        self.selected = selected.and_then(|(plugin, id)| self.items.iter().position(|i| i.plugin == plugin && i.id == id)).unwrap_or(0);
    }

    pub fn finished(&mut self, generation: u64) {
        if generation != self.generation {
            return;
        }
        self.pending = self.pending.saturating_sub(1);
        if self.pending == 0 {
            let held = std::mem::take(&mut self.held);
            if self.stale {
                self.items.clear();
                self.selected = 0;
                self.stale = false;
            }
            self.publish(held);
        }
    }

    fn select(&mut self, index: usize) {
        self.selected = index;
        self.pointed = None;
        self.selection_moved = true;
        self.action = 0;
        self.scroll_to_selected = true;
    }

    fn step(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let last = self.items.len() as isize - 1;
        let index = (self.selected as isize + delta).clamp(0, last) as usize;
        self.expanded |= index >= VISIBLE_ROWS;
        self.select(index);
    }

    fn visible_rows(&self) -> usize {
        if self.expanded { EXPANDED_ROWS } else { VISIBLE_ROWS }
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
            if let Some(question) = &run.confirm {
                self.confirming = Some(Confirming { question: question.clone(), action: run.clone(), item: item.clone(), on_action: true });
            } else {
                requests.push(Request::Run(run.clone(), item.clone()));
            }
        } else if let Some(completion) = &item.completion {
            let completion = completion.clone();
            self.set_input(completion);
        }
    }

    pub fn activate_selected(&mut self) -> Vec<Request> {
        let mut requests = Vec::new();
        if self.confirming.is_some() {
            self.answer(&mut requests);
        } else {
            self.activate(0, &mut requests);
        }
        requests
    }

    /// Enter in the question, on whichever button the keyboard is.
    fn answer(&mut self, requests: &mut Vec<Request>) {
        if let Some(confirming) = self.confirming.take()
            && confirming.on_action
        {
            requests.push(Request::Run(confirming.action, confirming.item));
        }
    }

    fn confirm_keys(&mut self, ctx: &egui::Context, requests: &mut Vec<Request>) {
        let pressed = |modifiers, key| ctx.input_mut(|i| i.consume_key(modifiers, key));
        if pressed(Modifiers::NONE, Key::Escape) {
            self.confirming = None;
            return;
        }
        let moved = pressed(Modifiers::NONE, Key::ArrowLeft)
            | pressed(Modifiers::NONE, Key::ArrowRight)
            | pressed(Modifiers::NONE, Key::Tab)
            | pressed(Modifiers::SHIFT, Key::Tab);
        if moved && let Some(confirming) = &mut self.confirming {
            confirming.on_action = !confirming.on_action;
        }
        // A real press: holding Enter from the first one mustn't answer.
        let enter = ctx.input(|i| {
            i.events.iter().any(|event| matches!(event, egui::Event::Key { key: Key::Enter, pressed: true, repeat: false, .. }))
        });
        pressed(Modifiers::NONE, Key::Enter);
        if enter {
            self.answer(requests);
        }
    }

    fn draw_confirm(&mut self, ui: &mut egui::Ui, requests: &mut Vec<Request>) {
        let Some(confirming) = &mut self.confirming else { return };
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, INPUT_HEIGHT), Sense::hover());
        let question = galley(ui, &confirming.question, INPUT_FONT, TEXT, width - INPUT_PADDING * 2.0);
        ui.painter().galley(egui::pos2(rect.left() + INPUT_PADDING + 4.0, rect.center().y - question.size().y / 2.0), question, TEXT);
        let (row, _) = ui.allocate_exact_size(Vec2::new(width, ROW_HEIGHT), Sense::hover());
        let half = (width - ROW_SPACING) / 2.0;
        let cancel = Rect::from_min_size(row.min, Vec2::new(half, ROW_HEIGHT));
        let run = Rect::from_min_size(row.min + Vec2::new(half + ROW_SPACING, 0.0), Vec2::new(half, ROW_HEIGHT));
        let mut clicked = None;
        for (rect, label, is_action) in [(cancel, "Cancel", false), (run, confirming.action.label.as_str(), true)] {
            let response = ui.interact(rect, ui.scope_id().with(is_action), Sense::click());
            let fill = if confirming.on_action == is_action {
                SELECTION
            } else if response.hovered() {
                Color32::from_white_alpha(10)
            } else {
                INPUT
            };
            ui.painter().rect_filled(rect, CornerRadius::same(11), fill);
            let text = galley(ui, label, TITLE_FONT, TEXT, rect.width());
            ui.painter().galley(rect.center() - text.size() / 2.0, text, TEXT);
            if response.clicked() {
                clicked = Some(is_action);
            }
        }
        if let Some(is_action) = clicked {
            confirming.on_action = is_action;
            self.answer(requests);
        }
    }

    fn keys(&mut self, ctx: &egui::Context, requests: &mut Vec<Request>) {
        if self.confirming.is_some() {
            return self.confirm_keys(ctx, requests);
        }
        let pressed = |modifiers, key| ctx.input_mut(|i| i.consume_key(modifiers, key));
        if pressed(Modifiers::NONE, Key::Escape) {
            requests.push(Request::Hide);
        }
        // Alt+Enter runs the second action, like Alt+click: New window, for an app.
        // Alt+Up or Alt+Down opens the whole list, starting on that same action.
        let actions = self.items.get(self.selected).map_or(0, |i| i.actions.len());
        let down = pressed(Modifiers::ALT, Key::ArrowDown);
        let up = pressed(Modifiers::ALT, Key::ArrowUp);
        if (down || up) && !self.actions_open {
            self.actions_open = true;
            self.action = second_action(actions);
        } else if down {
            self.action = (self.action + 1).min(actions.saturating_sub(1));
        } else if up {
            self.action = self.action.saturating_sub(1);
        }
        if pressed(Modifiers::ALT, Key::Enter) {
            let action = if self.actions_open { self.action } else { second_action(actions) };
            self.activate(action, requests);
        }
        if pressed(Modifiers::NONE, Key::Enter) {
            // Still showing the previous query's results: not what was typed.
            self.enter_waiting = self.stale;
            if !self.stale {
                self.activate(0, requests);
            }
        }
        if self.enter_waiting && !self.stale {
            self.enter_waiting = false;
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
            self.step(self.visible_rows() as isize);
        }
        if pressed(Modifiers::NONE, Key::PageUp) {
            self.step(-(self.visible_rows() as isize));
        }
        let completion = self.items.get(self.selected).and_then(|i| i.completion.clone());
        if let Some(completion) = completion.clone()
            && pressed(Modifiers::NONE, Key::Tab)
        {
            self.set_input(completion);
        }
        // Right at the end of the input does what Tab does, `/a` → `/amazon `.
        // Anywhere else it moves the cursor, so it's only taken when it completes.
        if let Some(completion) = completion.filter(|c| *c != self.input)
            && self.cursor_at_end(ctx)
            && pressed(Modifiers::NONE, Key::ArrowRight)
        {
            self.set_input(completion);
        }
    }

    /// Nothing selected and nothing after the cursor.
    fn cursor_at_end(&self, ctx: &egui::Context) -> bool {
        let Some(range) = self.input_id.and_then(|id| egui::TextEdit::load_state(ctx, id)).and_then(|s| s.cursor.char_range()) else {
            return true;
        };
        range.is_empty() && range.primary == CCursor::new(self.input.chars().count())
    }

    /// Draws a frame. Returns what to do, and where the panel and the card
    /// are: everything around them, the shadows included, lets clicks through.
    pub fn draw(&mut self, ui: &mut egui::Ui, icons: &mut Icons, previews: &mut Previews) -> (Vec<Request>, Vec<Rect>) {
        let ctx = ui.ctx().clone();
        let mut requests = Vec::new();
        self.keys(&ctx, &mut requests);
        let alt = ctx.input(|i| i.modifiers.alt);
        if !alt {
            self.actions_open = false;
            self.action = 0;
        }

        let frame = egui::Frame::new()
            .fill(WINDOW)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(CornerRadius::same(18))
            .inner_margin(PADDING)
            .shadow(egui::Shadow { offset: [0, 8], blur: 60, spread: 0, color: Color32::from_black_alpha(130) });

        let mut hovered_row = None;
        let room = Rect::from_min_size(egui::pos2(SIDE, SHADOW), ui.available_size() - Vec2::new(SIDE * 2.0, SHADOW));
        let mut panel_ui = ui.new_child(egui::UiBuilder::new().max_rect(room));
        let panel = frame.show(&mut panel_ui, |ui| {
            ui.set_width(WIDTH - PADDING * 2.0 - 2.0);
            ui.spacing_mut().item_spacing = Vec2::new(0.0, SPACING);
            if self.confirming.is_some() {
                self.draw_confirm(ui, &mut requests);
                return;
            }
            self.draw_input(ui);
            if !self.items.is_empty() {
                if self.actions_open {
                    self.draw_actions(ui, &mut requests);
                } else {
                    hovered_row = self.draw_rows(ui, icons, previews, alt, &mut requests);
                }
            }
        });
        let panel = panel.response.rect;
        let mut rects = vec![panel];
        if self.confirming.is_some() || self.actions_open || self.items.is_empty() {
            self.pointed = None;
            self.card = None;
            previews.player.follow(&ctx, None, 0);
            return (requests, rects);
        }
        // Moving off the rows and the card gives the card back to the selection.
        if ctx.input(|i| i.pointer.delta() != Vec2::ZERO) {
            let on_card = ctx.pointer_hover_pos().zip(self.card).is_some_and(|(at, card)| card.contains(at));
            if hovered_row.is_some() || !on_card {
                self.pointed = hovered_row;
            }
        }
        self.card = self.draw_card(ui, panel, icons, previews, &mut requests);
        if self.card.is_none() {
            previews.player.follow(&ctx, None, 0);
        }
        rects.extend(self.card);
        (requests, rects)
    }

    /// The pointed or selected file, large, beside the list. Its icon while
    /// the preview comes, or when it can't have one.
    fn draw_card(&mut self, ui: &mut egui::Ui, panel: Rect, icons: &mut Icons, previews: &mut Previews, requests: &mut Vec<Request>) -> Option<Rect> {
        let index = self.pointed.filter(|i| *i < self.items.len()).unwrap_or(self.selected);
        let item = self.items.get(index)?;
        let preview = item.preview.as_ref()?;
        let rect = Rect::from_min_size(egui::pos2(panel.right() + CARD_GAP, panel.top()), Vec2::new(CARD_WIDTH, panel_height()));
        let corner = CornerRadius::same(18);
        let shadow = egui::Shadow { offset: [0, 6], blur: 30, spread: 0, color: Color32::from_black_alpha(110) };
        ui.painter().add(shadow.as_shape(rect, corner));
        ui.painter().rect(rect, corner, WINDOW, Stroke::new(1.0, BORDER), egui::StrokeKind::Inside);

        let side = CARD_WIDTH - PADDING * 2.0;
        let well = Rect::from_min_size(rect.min + Vec2::splat(PADDING), Vec2::splat(side));
        ui.painter().rect_filled(well, CornerRadius::same(11), INPUT);
        let ppp = ui.ctx().pixels_per_point();
        let picture = well.shrink(ROW_PADDING);
        previews.player.follow(ui.ctx(), Some(&preview.path), (picture.width() * ppp) as u32);
        let video = previews.player.video(ui.ctx());
        if let Some(shown) = video.or_else(|| previews.get(ui.ctx(), preview, picture.width() * ppp)) {
            let at = previews::fit(picture, shown.size, ppp);
            egui::Image::from_texture(egui::load::SizedTexture::new(shown.texture, at.size())).corner_radius(4).paint_at(ui, at);
        } else if let Some(texture) = icons.get(&item.icon) {
            let at = Rect::from_center_size(well.center(), Vec2::splat(ICON * 2.0));
            ui.painter().image(texture, at, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        }
        if let Some(progress) = previews.player.progress() {
            draw_sound(ui.painter(), well, progress);
        }

        let width = side - ROW_PADDING;
        let left = well.left() + ROW_PADDING / 2.0;
        let title = galley(ui, &item.title, TITLE_FONT, TEXT, width);
        let subtitle = galley(ui, &item.subtitle, SUBTITLE_FONT, SUBTEXT, width);
        let total = title.size().y + subtitle.size().y;
        let top = (well.bottom() + rect.bottom() - total) / 2.0;
        let below = top + title.size().y;
        ui.painter().galley(egui::pos2(left, top), title, TEXT);
        ui.painter().galley(egui::pos2(left, below), subtitle, SUBTEXT);

        if ui.interact(rect, ui.scope_id().with("card"), Sense::click()).clicked() {
            self.select(index);
            self.activate(0, requests);
        }
        Some(rect)
    }

    fn draw_input(&mut self, ui: &mut egui::Ui) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), INPUT_HEIGHT), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(11), INPUT);
        let inner = rect.shrink2(Vec2::new(INPUT_PADDING + 4.0, INPUT_PADDING));
        let edit = egui::TextEdit::singleline(&mut self.input)
            .id_salt("input")
            .font(FontId::proportional(INPUT_FONT))
            .text_color(TEXT)
            // egui lays the hint out in the theme's body font, not `font`, and
            // puts the cursor at the hint's top left: at body size both sat
            // high and small. Sized like the text, the hint, the cursor and
            // what's typed line up.
            .hint_text(egui::RichText::new("Search, or / for commands").size(INPUT_FONT).color(SUBTEXT))
            .align(egui::Align2::LEFT_CENTER)
            .frame(egui::Frame::NONE)
            .desired_width(inner.width())
            .margin(Vec2::ZERO)
            .lock_focus(true);
        let response = ui.put(inner, edit);
        self.input_id = Some(response.id);
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

    /// Returns the row under the pointer.
    fn draw_rows(&mut self, ui: &mut egui::Ui, icons: &mut Icons, previews: &mut Previews, alt: bool, requests: &mut Vec<Request>) -> Option<usize> {
        let height = rows_height(self.visible_rows());
        let scroll_to = std::mem::take(&mut self.scroll_to_selected);
        let mut clicked = None;
        let mut hovered = None;
        let ppp = ui.ctx().pixels_per_point();
        let scrolled = egui::ScrollArea::vertical()
            .max_height(height)
            .auto_shrink([false, true])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(0.0, ROW_SPACING);
                for (index, item) in self.items.iter().enumerate() {
                    let selected = index == self.selected;
                    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::click());
                    // Before skipping what's out of sight: that's where the
                    // keyboard takes the selection.
                    if selected && scroll_to {
                        ui.scroll_to_rect(rect, None);
                    }
                    if !ui.is_rect_visible(rect) {
                        continue;
                    }
                    if selected {
                        ui.painter().rect_filled(rect, CornerRadius::same(11), SELECTION);
                    } else if response.hovered() {
                        ui.painter().rect_filled(rect, CornerRadius::same(11), Color32::from_white_alpha(10));
                    }
                    if response.hovered() {
                        hovered = Some(index);
                    }
                    let icon_rect = Rect::from_min_size(rect.min + Vec2::splat(ROW_PADDING), Vec2::splat(ICON));
                    let shown = item.preview.as_ref().and_then(|p| previews.get(ui.ctx(), p, ICON * ppp));
                    if let Some(shown) = shown {
                        let at = previews::fit(icon_rect, shown.size, ppp);
                        egui::Image::from_texture(egui::load::SizedTexture::new(shown.texture, at.size())).corner_radius(3).paint_at(ui, at);
                    } else if let Some(texture) = icons.get(&item.icon) {
                        let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                        ui.painter().image(texture, icon_rect, uv, Color32::WHITE);
                    }
                    let left = icon_rect.right() + ICON_GAP;
                    let width = rect.right() - ROW_PADDING - left;
                    let title = galley(ui, &item.title, TITLE_FONT, TEXT, width);
                    // Holding Alt: what Alt+Enter would do, in place of the usual text.
                    let hint = (alt && selected && item.actions.len() > 1)
                        .then(|| format!("Alt+Enter: {}  ·  Alt+Down: all actions", item.actions[second_action(item.actions.len())].label));
                    let subtitle = hint.as_deref().unwrap_or(&item.subtitle);
                    if subtitle.is_empty() {
                        ui.painter().galley(egui::pos2(left, rect.center().y - title.size().y / 2.0), title, TEXT);
                    } else {
                        let subtitle = galley(ui, subtitle, SUBTITLE_FONT, SUBTEXT, width);
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
        // What's below the last row in sight.
        let pitch = ROW_HEIGHT + ROW_SPACING;
        let seen = ((scrolled.state.clamped_offset().y + scrolled.inner_rect.height() + ROW_SPACING) / pitch + 0.01).floor() as usize;
        let more = self.items.len().saturating_sub(seen);
        self.more = more;
        if more > 0 && self.draw_more(ui, more) {
            self.expanded = true;
        }
        if let Some(index) = clicked {
            self.select(index);
            let actions = self.items[index].actions.len();
            self.activate(if alt { second_action(actions) } else { 0 }, requests);
        }
        hovered
    }

    /// "12 more", under the list. Clicking it shows twelve rows.
    fn draw_more(&self, ui: &mut egui::Ui, more: usize) -> bool {
        let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), MORE_HEIGHT), Sense::click());
        let color = if response.hovered() && !self.expanded { TEXT } else { SUBTEXT };
        let text = galley(ui, &format!("{more} more"), SUBTITLE_FONT, color, rect.width() - ROW_PADDING * 2.0);
        ui.painter().galley(egui::pos2(rect.left() + ROW_PADDING, rect.center().y - text.size().y / 2.0), text, color);
        response.clicked() && !self.expanded
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

/// Over a playing sound's preview: a play mark, and how far along it is,
/// as Fractale draws it.
fn draw_sound(painter: &egui::Painter, well: Rect, progress: f32) {
    let centre = well.left_top() + Vec2::splat(18.0);
    painter.circle_filled(centre, 12.0, WINDOW.gamma_multiply(0.85));
    let mark = vec![centre + Vec2::new(-4.0, -6.0), centre + Vec2::new(6.5, 0.0), centre + Vec2::new(-4.0, 6.0)];
    painter.add(egui::Shape::convex_polygon(mark, TEXT, Stroke::NONE));
    let bar = Rect::from_min_size(egui::pos2(well.left() + 11.0, well.bottom() - 7.0), Vec2::new(well.width() - 22.0, 3.0));
    painter.rect_filled(bar, CornerRadius::same(2), BORDER);
    painter.rect_filled(Rect::from_min_size(bar.min, Vec2::new(bar.width() * progress.clamp(0.0, 1.0), 3.0)), CornerRadius::same(2), TEXT);
}

/// What Alt runs: the second action, or the only one.
fn second_action(actions: usize) -> usize {
    if actions > 1 { 1 } else { 0 }
}

/// By tier, then best first.
fn sort(items: &mut [Item]) {
    items.sort_by(|a, b| a.tier.cmp(&b.tier).then(b.score.total_cmp(&a.score)));
}

/// One line, cut with an ellipsis when too long.
pub fn galley(ui: &egui::Ui, text: &str, size: f32, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::Plugin;
    use crate::plugin::Query;
    use std::sync::Arc;

    struct Regular;

    impl Plugin for Regular {
        fn id(&self) -> &'static str {
            "regular"
        }
        fn regular(&self) -> bool {
            true
        }
        fn query(&self, _: &Query) {}
    }

    fn launcher() -> Launcher {
        // Two plugins, so `pending` is 2. Nobody reads the events.
        let (events, _) = std::sync::mpsc::channel();
        let plugins: Vec<Arc<dyn Plugin>> = vec![Arc::new(Regular), Arc::new(Regular)];
        Launcher::new(Runner::new(plugins, events), History::in_memory())
    }

    fn item(id: &str, tier: u8) -> Item {
        let mut item = Item::new(id, id, "", crate::plugin::Icon::Theme("x".into())).tier(tier);
        item.plugin = "regular";
        item
    }

    fn ids(launcher: &Launcher) -> Vec<&str> {
        launcher.items.iter().map(|i| i.id.as_str()).collect()
    }

    #[test]
    fn google_waits_for_the_others() {
        let mut launcher = launcher();
        launcher.set_input("projects".into());
        let generation = launcher.generation;
        launcher.results(generation, vec![item("google", LAST_TIER)]);
        assert!(launcher.items.is_empty() && launcher.stale, "Google alone isn't shown");
        launcher.finished(generation);
        assert!(launcher.stale, "one plugin hasn't finished");
        launcher.results(generation, vec![item("folder", 1)]);
        assert_eq!(ids(&launcher), ["folder"]);
        launcher.finished(generation);
        assert_eq!(ids(&launcher), ["folder", "google"]);
        assert_eq!(launcher.selected, 0);
    }

    #[test]
    fn google_alone_appears_once_everyone_has_finished() {
        let mut launcher = launcher();
        launcher.set_input("zzz".into());
        let generation = launcher.generation;
        launcher.results(generation, vec![item("google", LAST_TIER)]);
        launcher.finished(generation);
        launcher.finished(generation);
        assert_eq!(ids(&launcher), ["google"]);
        assert!(!launcher.stale);
    }

    fn asking(launcher: &mut Launcher) -> Vec<Request> {
        launcher.set_input("/shutdown".into());
        let generation = launcher.generation;
        let run = Action::new("run", "Shut down", crate::plugin::Effect::Open("x".into())).confirm("Shut down?");
        launcher.results(generation, vec![item("shutdown", 0).action(run)]);
        launcher.activate_selected()
    }

    #[test]
    fn the_first_enter_only_asks_and_the_second_runs() {
        let mut launcher = launcher();
        assert!(asking(&mut launcher).is_empty());
        assert!(launcher.confirming.as_ref().unwrap().on_action);
        assert!(matches!(launcher.activate_selected().as_slice(), [Request::Run(..)]));
        assert!(launcher.confirming.is_none());
    }

    #[test]
    fn cancel_runs_nothing() {
        let mut launcher = launcher();
        asking(&mut launcher);
        launcher.confirming.as_mut().unwrap().on_action = false;
        assert!(launcher.activate_selected().is_empty());
        assert!(launcher.confirming.is_none());
    }

    #[test]
    fn hiding_forgets_the_question() {
        let mut launcher = launcher();
        asking(&mut launcher);
        launcher.reset();
        assert!(launcher.confirming.is_none());
    }

    #[test]
    fn alt_runs_the_second_action_when_there_is_one() {
        assert_eq!(second_action(0), 0);
        assert_eq!(second_action(1), 0);
        assert_eq!(second_action(3), 1);
    }

    #[test]
    fn going_down_past_five_shows_twelve_until_the_query_changes() {
        let mut launcher = launcher();
        launcher.set_input("a".into());
        let generation = launcher.generation;
        launcher.results(generation, (0..12).map(|i| item(&i.to_string(), 0)).collect());
        launcher.step(4);
        assert!(!launcher.expanded, "the fifth row is in sight");
        launcher.step(1);
        assert!(launcher.expanded);
        assert_eq!(launcher.visible_rows(), EXPANDED_ROWS);
        launcher.step(-5);
        assert!(launcher.expanded, "going back up keeps twelve");
        launcher.set_input("ab".into());
        assert!(!launcher.expanded);
    }

    #[test]
    fn the_list_scrolls_to_follow_the_keyboard() {
        let mut launcher = launcher();
        launcher.set_input("a".into());
        let generation = launcher.generation;
        launcher.results(generation, (0..30).map(|i| item(&i.to_string(), 0)).collect());
        let (events, _) = std::sync::mpsc::channel();
        let mut icons = Icons::new(events.clone(), crate::icons::Window::Launcher);
        let mut previews = Previews::new(events);
        let ctx = egui::Context::default();
        let (width, height) = surface_size();
        let mut frame = |launcher: &mut Launcher, time: f64| {
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(width as f32, height as f32))),
                time: Some(time),
                ..Default::default()
            };
            let mut output = ctx.run_ui(raw, |ui| {
                launcher.draw(ui, &mut icons, &mut previews);
            });
            output.textures_delta.clear();
        };
        frame(&mut launcher, 0.0);
        assert_eq!(launcher.more, 25, "five in sight");
        launcher.step(20);
        for time in 1..10 {
            frame(&mut launcher, time as f64);
        }
        assert_eq!(launcher.more, 9, "twelve in sight, down to the selected 21st");
    }

    #[test]
    fn old_results_are_dropped() {
        let mut launcher = launcher();
        launcher.set_input("a".into());
        let old = launcher.generation;
        launcher.set_input("ab".into());
        launcher.results(old, vec![item("old", 0)]);
        assert!(launcher.items.is_empty());
    }
}
