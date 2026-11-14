//! Emoji picker popup for reacting to a message

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Most common reactions, shown first
pub const COMMON_REACTIONS: &[&str] = &[
    "👍", "❤️", "😂", "😮", "😢", "🙏", "👎", "🔥", "🎉", "😍", "🤔", "👏", "😅", "💯", "✅", "🙌",
    "😊", "😡", "🤣", "👀", "💪", "🥰", "😎", "🤷",
];

/// Number of emoji per row in the picker grid
pub const COLUMNS: usize = 8;

const MAX_SEARCH_RESULTS: usize = COLUMNS * 5;

#[derive(Debug, PartialEq, Eq)]
pub enum PickerAction {
    None,
    Cancel,
    Pick(String),
}

#[derive(Debug, Default)]
pub struct ReactionPicker {
    /// Index into `items()`; `items().len()` is the search cell
    pub selected: usize,
    pub query: String,
}

impl ReactionPicker {
    /// Emoji currently shown in the grid (common ones or search results)
    pub fn items(&self) -> Vec<&'static str> {
        if self.query.is_empty() {
            COMMON_REACTIONS.to_vec()
        } else {
            search_emoji(&self.query)
        }
    }

    pub fn is_search_selected(&self) -> bool {
        self.selected >= self.items().len()
    }

    fn select_search(&mut self) {
        self.selected = self.items().len();
    }

    fn move_by(&mut self, delta: isize) {
        let len = self.items().len() + 1; // + search cell
        let next = (self.selected as isize + delta).clamp(0, len as isize - 1);
        self.selected = next as usize;
    }

    pub fn on_key(&mut self, key: KeyEvent) -> PickerAction {
        let on_search = self.is_search_selected();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return PickerAction::Cancel,
            KeyCode::Char('c') if ctrl => return PickerAction::Cancel,
            KeyCode::Enter => {
                let items = self.items();
                return match items.get(self.selected).or_else(|| items.first()) {
                    Some(emoji) => PickerAction::Pick(emoji.to_string()),
                    None => PickerAction::None,
                };
            }
            KeyCode::Left => self.move_by(-1),
            KeyCode::Right => self.move_by(1),
            KeyCode::Up => self.move_by(-(COLUMNS as isize)),
            KeyCode::Down => self.move_by(COLUMNS as isize),
            KeyCode::Tab => self.move_by(1),
            KeyCode::BackTab => self.move_by(-1),
            KeyCode::Backspace if on_search => {
                self.query.pop();
                self.select_search();
            }
            // On the grid, hjkl navigate; on the search cell they are typed
            KeyCode::Char('h') if !on_search => self.move_by(-1),
            KeyCode::Char('l') if !on_search => self.move_by(1),
            KeyCode::Char('k') if !on_search => self.move_by(-(COLUMNS as isize)),
            KeyCode::Char('j') if !on_search => self.move_by(COLUMNS as isize),
            KeyCode::Char('/') if !on_search => self.select_search(),
            KeyCode::Char(c) if on_search && !ctrl => {
                self.query.push(c);
                self.select_search();
            }
            _ => {}
        }
        PickerAction::None
    }
}

/// Finds emoji by name or shortcode, e.g. "shrug" => 🤷
pub fn search_emoji(query: &str) -> Vec<&'static str> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let mut exact = Vec::new();
    let mut prefix = Vec::new();
    let mut contains = Vec::new();
    for emoji in emojis::iter() {
        let name = emoji.name().to_lowercase();
        let shortcodes: Vec<&str> = emoji.shortcodes().collect();
        if shortcodes.contains(&query.as_str()) || name == query {
            exact.push(emoji.as_str());
        } else if shortcodes.iter().any(|s| s.starts_with(&query)) || name.starts_with(&query) {
            prefix.push(emoji.as_str());
        } else if shortcodes.iter().any(|s| s.contains(&query)) || name.contains(&query) {
            contains.push(emoji.as_str());
        }
    }
    exact
        .into_iter()
        .chain(prefix)
        .chain(contains)
        .take(MAX_SEARCH_RESULTS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn pick_common() {
        let mut picker = ReactionPicker::default();
        picker.on_key(key(KeyCode::Char('l')));
        assert_eq!(
            picker.on_key(key(KeyCode::Enter)),
            PickerAction::Pick("❤️".into())
        );
    }

    #[test]
    fn search_from_end_of_list() {
        let mut picker = ReactionPicker::default();
        // move to the very end: the search cell
        for _ in 0..10 {
            picker.on_key(key(KeyCode::Down));
        }
        assert!(picker.is_search_selected());
        for c in "shrug".chars() {
            picker.on_key(key(KeyCode::Char(c)));
        }
        assert_eq!(picker.query, "shrug");
        // enter on the search cell picks the best match
        assert_eq!(
            picker.on_key(key(KeyCode::Enter)),
            PickerAction::Pick("🤷".into())
        );
    }

    #[test]
    fn search_emoji_finds_by_name() {
        assert_eq!(search_emoji("thumbsup").first(), Some(&"👍"));
        assert!(search_emoji("rocket").contains(&"🚀"));
        assert!(search_emoji("").is_empty());
    }
}
