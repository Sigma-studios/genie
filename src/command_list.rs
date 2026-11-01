//! The window `/list` opens: every command, by group, with a search bar.

use crate::icons::Icons;
use crate::launcher;
use crate::matcher;
use crate::plugin::Command;
use crate::plugin::Icon;
use egui::CornerRadius;
use egui::FontId;
use egui::Key;
use egui::Modifiers;
use egui::Sense;
use egui::Vec2;

const MARGIN: f32 = 12.0;
const SEARCH_FONT: f32 = 18.0;
const SEARCH_HEIGHT: f32 = 40.0;
const ROW_HEIGHT: f32 = 46.0;
const ICON: f32 = 30.0;
const HEADING_HEIGHT: f32 = 30.0;
const NAME_FONT: f32 = 16.0;
const DESCRIPTION_FONT: f32 = 12.0;
const HEADING_FONT: f32 = 13.0;

pub enum Outcome {
    Close,
    /// Try this command in the launcher.
    Run(String),
}

pub struct CommandList {
    groups: Vec<(String, Vec<Command>)>,
    search: String,
    selected: usize,
    scroll_to_selected: bool,
}

/// The groups with what matches. A group's own name matches all of it, so
/// `spotify` lists every Spotify command.
fn filter(groups: &[(String, Vec<Command>)], search: &str) -> Vec<(String, Vec<Command>)> {
    let search = search.trim().trim_start_matches('/');
    groups
        .iter()
        .filter_map(|(group, commands)| {
            let found: Vec<Command> = commands
                .iter()
                .filter(|command| {
                    let mut fields = vec![(command.name.as_str(), 1.0), (command.description.as_str(), 1.0), (group.as_str(), 1.0)];
                    fields.extend(command.aliases.iter().map(|a| (a.as_str(), 1.0)));
                    search.is_empty() || matcher::score(search, &fields).is_some()
                })
                .cloned()
                .collect();
            (!found.is_empty()).then(|| (group.clone(), found))
        })
        .collect()
}

impl CommandList {
    pub fn new(groups: Vec<(String, Vec<Command>)>) -> Self {
        Self { groups, search: String::new(), selected: 0, scroll_to_selected: false }
    }

    pub fn draw(&mut self, ui: &mut egui::Ui, icons: &mut Icons) -> Option<Outcome> {
        let ctx = ui.ctx().clone();
        let pressed = |key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, key));
        let mut outcome = None;
        if pressed(Key::Escape) {
            outcome = Some(Outcome::Close);
        }
        let groups = filter(&self.groups, &self.search);
        let count: usize = groups.iter().map(|(_, commands)| commands.len()).sum();
        if pressed(Key::ArrowDown) {
            self.selected += 1;
            self.scroll_to_selected = true;
        }
        if pressed(Key::ArrowUp) {
            self.selected = self.selected.saturating_sub(1);
            self.scroll_to_selected = true;
        }
        self.selected = self.selected.min(count.saturating_sub(1));
        let enter = pressed(Key::Enter);

        egui::Frame::new().fill(launcher::WINDOW).inner_margin(MARGIN).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            self.draw_search(ui);
            ui.add_space(MARGIN / 2.0);
            let scroll_to = std::mem::take(&mut self.scroll_to_selected);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(0.0, 2.0);
                let mut index = 0;
                for (group, commands) in &groups {
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), HEADING_HEIGHT), Sense::hover());
                    let heading = launcher::galley(ui, group, HEADING_FONT, launcher::SUBTEXT, rect.width());
                    ui.painter().galley(egui::pos2(rect.left() + 6.0, rect.bottom() - heading.size().y - 4.0), heading, launcher::SUBTEXT);
                    for command in commands {
                        let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::click());
                        let selected = index == self.selected;
                        if selected {
                            ui.painter().rect_filled(rect, CornerRadius::same(9), launcher::SELECTION);
                            if scroll_to {
                                ui.scroll_to_rect(rect, None);
                            }
                        } else if response.hovered() {
                            ui.painter().rect_filled(rect, CornerRadius::same(9), egui::Color32::from_white_alpha(10));
                        }
                        let icon_rect = egui::Rect::from_center_size(egui::pos2(rect.left() + 8.0 + ICON / 2.0, rect.center().y), Vec2::splat(ICON));
                        if let Some(texture) = icons.get(&Icon::Theme(command.icon.clone())) {
                            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                            ui.painter().image(texture, icon_rect, uv, egui::Color32::WHITE);
                        }
                        let left = icon_rect.right() + 10.0;
                        let width = rect.right() - left - 8.0;
                        let name = launcher::galley(ui, &title(command), NAME_FONT, launcher::TEXT, width);
                        let description = launcher::galley(ui, &command.detail(), DESCRIPTION_FONT, launcher::SUBTEXT, width);
                        let top = rect.center().y - (name.size().y + description.size().y) / 2.0;
                        let below = top + name.size().y;
                        ui.painter().galley(egui::pos2(left, top), name, launcher::TEXT);
                        ui.painter().galley(egui::pos2(left, below), description, launcher::SUBTEXT);
                        if response.clicked() || (enter && selected) {
                            outcome = Some(Outcome::Run(format!("/{} ", command.name)));
                        }
                        index += 1;
                    }
                }
            });
        });
        outcome
    }

    fn draw_search(&mut self, ui: &mut egui::Ui) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), SEARCH_HEIGHT), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(9), launcher::INPUT);
        let edit = egui::TextEdit::singleline(&mut self.search)
            .id_salt("search")
            .font(FontId::proportional(SEARCH_FONT))
            .text_color(launcher::TEXT)
            .hint_text(egui::RichText::new("Search commands").color(launcher::SUBTEXT))
            .frame(egui::Frame::NONE)
            .desired_width(rect.width() - 20.0)
            .margin(Vec2::ZERO)
            .lock_focus(true);
        let response = ui.put(rect.shrink2(Vec2::new(10.0, 8.0)), edit);
        response.request_focus();
        if response.changed() {
            self.selected = 0;
            self.scroll_to_selected = true;
        }
    }
}

/// `/spotify  (/sp, /play)`
fn title(command: &Command) -> String {
    if command.aliases.is_empty() {
        return format!("/{}", command.name);
    }
    let aliases: Vec<String> = command.aliases.iter().map(|a| format!("/{a}")).collect();
    format!("/{}  ({})", command.name, aliases.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn groups() -> Vec<(String, Vec<Command>)> {
        vec![
            ("Spotify".into(), vec![Command::new("next", &[], "Skip the track", "a"), Command::new("queue", &[], "Add a song", "b")]),
            ("Web search".into(), vec![Command::new("youtube", &["yt"], "Search YouTube", "c")]),
        ]
    }

    fn names(search: &str) -> Vec<String> {
        filter(&groups(), search).into_iter().flat_map(|(_, commands)| commands).map(|c| c.name).collect()
    }

    #[test]
    fn searching_keeps_groups_and_finds_by_any_field() {
        assert_eq!(filter(&groups(), "").len(), 2);
        assert_eq!(names("song"), ["queue"]);
        assert_eq!(names("yt"), ["youtube"]);
        assert_eq!(names("/queue"), ["queue"]);
        assert!(names("zzz").is_empty());
    }

    #[test]
    fn a_groups_name_lists_all_of_it() {
        assert_eq!(names("spotify"), ["next", "queue"]);
        let found = filter(&groups(), "song");
        assert_eq!(found[0].0, "Spotify");
    }

    #[test]
    fn titles_show_aliases() {
        assert_eq!(title(&Command::new("youtube", &["yt"], "", "")), "/youtube  (/yt)");
        assert_eq!(title(&Command::new("queue", &[], "", "")), "/queue");
    }
}
