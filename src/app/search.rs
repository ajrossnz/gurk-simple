//! Find messages in the current channel

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;
use uuid::Uuid;

use crate::data::ChannelId;
use crate::input::Input;

use super::window::PAGE;
use super::{App, Focus};

/// Maximum number of search results shown
const MAX_HITS: usize = 200;

pub(crate) struct SearchHit {
    pub arrived_at: u64,
    pub from_id: Uuid,
    pub text: String,
}

pub(crate) struct MessageSearch {
    pub channel_id: ChannelId,
    pub input: Input,
    /// Matching messages, newest first
    pub hits: Vec<SearchHit>,
    pub state: ListState,
}

impl App {
    pub(super) fn open_search(&mut self) {
        let Some(channel_id) = self.selected_channel_id() else {
            self.set_status("Select a channel first");
            return;
        };
        self.search = Some(MessageSearch {
            channel_id,
            input: Input::default(),
            hits: Vec::new(),
            state: ListState::default(),
        });
    }

    fn update_search_hits(&mut self) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        let query = search.input.data.trim().to_lowercase();
        search.hits.clear();
        if !query.is_empty() {
            let hits = self
                .storage
                .messages(search.channel_id)
                .rev()
                .filter(|m| !m.deleted)
                .filter_map(|m| {
                    let text = m.message.as_deref().unwrap_or_default();
                    let matches = text.to_lowercase().contains(&query)
                        || m.attachments.iter().any(|a| {
                            a.filename
                                .file_name()
                                .is_some_and(|f| f.to_string_lossy().to_lowercase().contains(&query))
                        });
                    matches.then(|| SearchHit {
                        arrived_at: m.arrived_at,
                        from_id: m.from_id,
                        text: text.replace('\n', " "),
                    })
                })
                .take(MAX_HITS);
            search.hits.extend(hits);
        }
        search
            .state
            .select((!search.hits.is_empty()).then_some(0));
    }

    pub(super) fn on_search_key(&mut self, key: KeyEvent) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let len = search.hits.len();
        match key.code {
            KeyCode::Esc => self.search = None,
            KeyCode::Char('c' | 'f') if ctrl => self.search = None,
            KeyCode::Up => {
                let i = search.state.selected().unwrap_or(0).saturating_sub(1);
                search.state.select((len > 0).then_some(i));
            }
            KeyCode::Down => {
                let i = (search.state.selected().unwrap_or(0) + 1).min(len.saturating_sub(1));
                search.state.select((len > 0).then_some(i));
            }
            KeyCode::Enter => {
                let target = search
                    .state
                    .selected()
                    .and_then(|i| search.hits.get(i))
                    .map(|hit| (search.channel_id, hit.arrived_at));
                if let Some((channel_id, arrived_at)) = target {
                    self.search = None;
                    self.jump_to_message(channel_id, arrived_at);
                }
            }
            KeyCode::Backspace => {
                search.input.on_backspace();
                self.update_search_hits();
            }
            KeyCode::Left => search.input.on_left(),
            KeyCode::Right => search.input.on_right(),
            KeyCode::Char('w') if ctrl => {
                search.input.on_delete_word();
                self.update_search_hits();
            }
            KeyCode::Char(c) if !ctrl => {
                search.input.put_char(c);
                self.update_search_hits();
            }
            _ => {}
        }
    }

    /// Selects the given message in the messages pane, loading the messages around it
    pub(crate) fn jump_to_message(&mut self, channel_id: ChannelId, arrived_at: u64) {
        if self.selected_channel_id() != Some(channel_id) {
            return;
        }
        let pos = self.positions.entry(channel_id).or_default();
        pos.selected = Some(arrived_at);
        pos.viewport_bottom = Some(arrived_at);
        if let Some(window) = self.window.as_mut() {
            window.load_around(&*self.storage, arrived_at, PAGE);
        }
        self.focus = Focus::Messages;
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::app::tests::test_app;
    use crate::data::Message;

    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn search_and_jump() {
        let (mut app, _events, _sent) = test_app().await;
        let channel_id = app.channels().first().unwrap().id;
        let user_id = app.user_id;
        for (at, text) in [(10, "hello there"), (20, "nothing"), (30, "Hello again")] {
            app.storage
                .store_message(channel_id, &Message::text(user_id, at, text.to_owned()));
        }
        app.on_channel_changed();

        app.open_search();
        for c in "hello".chars() {
            app.on_search_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        let hits: Vec<u64> = app
            .search
            .as_ref()
            .unwrap()
            .hits
            .iter()
            .map(|h| h.arrived_at)
            .collect();
        assert_eq!(hits, [30, 10]);

        app.on_search_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.on_search_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.search.is_none());
        assert_eq!(app.focus, Focus::Messages);
        assert_eq!(app.positions[&channel_id].selected, Some(10));
    }
}
