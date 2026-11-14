use std::borrow::Cow;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use arboard::ImageData;
use chrono::{DateTime, Local, TimeZone};
use crokey::Combiner;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use image::codecs::png::PngEncoder;
use image::{ImageBuffer, ImageEncoder, Rgba};
use presage::libsignal_service::sender::AttachmentSpec;
use tracing::{error, info};

use crate::command::{
    Command, DirectionVertical, MoveAmountText, MoveAmountVisual, MoveDirection, Widget, WindowMode,
};
use crate::data::{ChannelId, Message};
use crate::file_browser::{
    BrowserAction, BrowserPurpose, FileBrowser, UiState, copy_into_dir, display_path,
    downloads_dir, home_dir,
};
use crate::image_preview::is_image;
use crate::reaction_picker::{PickerAction, ReactionPicker};
use crate::storage::MessageId;
use crate::util::{ATTACHMENT_REGEX, URL_REGEX};

use super::{App, Focus, HandleReactionOptions, open_file, open_url, to_emoji};

impl App {
    async fn on_command(&mut self, command: Command) -> anyhow::Result<()> {
        match command {
            Command::Help => self.toggle_help(),
            Command::MoveText(MoveDirection::Previous, MoveAmountText::Word) => {
                self.get_input().move_back_word()
            }
            Command::MoveText(MoveDirection::Previous, MoveAmountText::Character) => {
                self.get_input().on_left()
            }
            Command::MoveText(MoveDirection::Previous, MoveAmountText::Line) => {
                self.get_input().move_line_up()
            }
            Command::MoveText(MoveDirection::Next, MoveAmountText::Word) => {
                self.get_input().move_forward_word()
            }
            Command::MoveText(MoveDirection::Next, MoveAmountText::Character) => {
                self.get_input().on_right()
            }
            Command::MoveText(MoveDirection::Next, MoveAmountText::Line) => {
                self.get_input().move_line_down()
            }
            Command::SelectMessage(MoveDirection::Previous, MoveAmountVisual::Entry) => {
                self.on_pgup()
            }
            Command::SelectMessage(MoveDirection::Next, MoveAmountVisual::Entry) => self.on_pgdn(),
            Command::KillBackwardLine => self.get_input().on_delete_line(),
            Command::KillWord => self.get_input().on_delete_word(),
            Command::CopyMessage(_) => self.copy_selection(),
            Command::KillLine => self.get_input().on_delete_suffix(),
            Command::SelectChannel(MoveDirection::Previous) => self.select_previous_channel(),
            Command::SelectChannel(MoveDirection::Next) => self.select_next_channel(),
            Command::SelectChannelModal(MoveDirection::Previous) => self.select_channel_prev(),
            Command::SelectChannelModal(MoveDirection::Next) => self.select_channel_next(),
            Command::KillWholeLine => self.get_input().on_delete_line(),
            Command::BeginningOfLine => self.get_input().on_home(),
            Command::EndOfLine => self.get_input().on_end(),
            Command::EditMessage => {
                if self.start_editing().is_some() {
                    self.set_focus(Focus::Input);
                }
            }
            // Command::ReplyMessage => unimplemented!("{command:?}"),
            Command::DeleteMessage => {
                self.delete_selected_message();
            }
            Command::ToggleChannelModal => {
                if !self.select_channel.is_shown {
                    self.select_channel.reset(&*self.storage);
                    self.select_channel.title = "Jump to chat";
                }
                self.select_channel.is_shown = !self.select_channel.is_shown;
            }
            Command::ToggleMultiline => {}
            Command::NewLine => {
                self.set_focus(Focus::Input);
                self.input.new_line();
            }
            Command::FocusNext => self.focus_next(),
            Command::FocusPrevious => self.focus_previous(),
            Command::FocusInput => self.set_focus(Focus::Input),
            Command::ReactPicker => self.open_reaction_picker(),
            Command::Reply => self.start_reply(),
            Command::OpenMessage => self.open_selected_message(),
            Command::SaveAttachment => self.open_save_browser(),
            Command::AttachFile => self.open_attach_browser(),
            Command::ClearInput => self.clear_input(),
            Command::Search => self.open_search(),
            Command::NewMessage => self.open_new_message().await,
            Command::Paste => self.paste_from_clipboard(),
            Command::React(reaction) => {
                if let Some(channel_id) = self.selected_channel_id() {
                    self.add_reaction(channel_id, reaction).await;
                }
            }
            Command::OpenUrl => {
                self.try_open_url();
            }
            Command::OpenFile => {
                self.try_open_file();
            }
            Command::DeleteCharacter(MoveDirection::Previous) => {
                self.get_input().on_backspace();
            }
            Command::DeleteCharacter(MoveDirection::Next) => {
                self.get_input().on_delete();
            }
            Command::Quit => {
                self.should_quit = true;
            }
            Command::Redraw => {
                self.should_clear = true;
            }
            Command::Scroll(Widget::Help, DirectionVertical::Up, MoveAmountVisual::Entry) => {
                if self.help_scroll.0 >= 1 {
                    self.help_scroll.0 -= 1
                }
            }
            Command::Scroll(Widget::Help, DirectionVertical::Down, MoveAmountVisual::Entry) => {
                // TODO: prevent overscrolling
                self.help_scroll.0 += 1
            }
            Command::ToggleMuteChannel => {
                self.toggle_mute_channel();
                if let Some(channel) = self.selected_channel() {
                    let status = if channel.muted { "Muted" } else { "Unmuted" };
                    let status = format!("{status} {}", self.channel_name(channel));
                    self.set_status(status);
                }
            }
            Command::ToggleChannelList => self.toggle_channel_list(),
            Command::OpenEditor => {
                self.open_editor_requested = true;
            }
            Command::NoOp => {}
        }
        Ok(())
    }

    pub async fn on_key(&mut self, key: KeyEvent) -> anyhow::Result<()> {
        // popups capture all keys
        if self.reaction_picker.is_some() {
            self.on_reaction_picker_key(key).await;
            return Ok(());
        }
        if self.file_browser.is_some() {
            self.on_file_browser_key(key);
            return Ok(());
        }
        if self.search.is_some() {
            self.on_search_key(key);
            return Ok(());
        }

        let input_focused = self.focus == Focus::Input && !self.is_popup_shown();
        if key.code == KeyCode::Tab && input_focused && self.input.try_complete_emoji() {
            return Ok(());
        }
        // Tab not consumed or different key, stop any active completion cycle.
        self.input.completion_partial = None;

        if let Some(cmd) = self.event_to_command(&key) {
            self.on_command(cmd.clone()).await?;
            return Ok(());
        }

        let is_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.select_channel.is_shown {
            match key.code {
                KeyCode::Enter => {
                    if let Some(channel_id) = self.select_channel.selected_channel_id().copied() {
                        self.select_channel.is_shown = false;
                        self.select_channel_by_id(channel_id)?;
                        self.set_focus(Focus::Input);
                    }
                }
                KeyCode::Char(c) if !is_ctrl => self.select_channel.input.put_char(c),
                _ => {}
            }
            return Ok(());
        }

        match self.focus {
            Focus::Input => match key.code {
                KeyCode::Char('\r') => self.input.put_char('\n'),
                KeyCode::Enter => {
                    if (!self.input.data.is_empty() || !self.pending_attachments().is_empty())
                        && let Some(channel_id) = self.selected_channel_id()
                    {
                        self.send_input(channel_id);
                    }
                }
                KeyCode::Esc => {
                    if !self.reset_editing() && !self.cancel_reply() {
                        self.reset_message_selection();
                    }
                }
                KeyCode::Char(c) if !is_ctrl => {
                    self.input.put_char(c);
                    if c == ':' {
                        self.input.convert_emoji_on_colon();
                    }
                }
                _ => {}
            },
            Focus::Messages => {
                if key.code == KeyCode::Esc {
                    self.set_focus(Focus::Input);
                }
            }
            Focus::Channels => {}
        }
        Ok(())
    }

    fn select_channel_by_id(&mut self, channel_id: ChannelId) -> anyhow::Result<()> {
        let old = self.selected_channel_id();
        let idx = self
            .channels
            .items
            .iter()
            .position(|channel| channel.id == channel_id)
            .context("channel disappeared during channel select popup")?;
        self.channels.state.select(Some(idx));
        let new = self.selected_channel_id();
        self.swap_channel_draft(old, new);
        self.on_channel_changed();
        self.reset_unread_messages();
        Ok(())
    }

    /// Opens the contact/group picker to start a new conversation
    async fn open_new_message(&mut self) {
        // pick up contacts and groups which are not known yet
        crate::storage::sync_from_signal(&*self.signal_manager, &mut *self.storage).await;
        for channel in self.storage.channels() {
            if self.channel(channel.id).is_none() {
                self.channels.items.push(channel);
            }
        }
        self.select_channel.reset(&*self.storage);
        self.select_channel.title = "New message to (type to filter contacts and groups)";
        self.select_channel.is_shown = true;
    }

    fn open_reaction_picker(&mut self) {
        if self.selected_message_id().is_none() {
            self.set_status("Select a message first (Tab to the messages pane)");
            return;
        }
        self.reaction_picker = Some(ReactionPicker::default());
    }

    async fn on_reaction_picker_key(&mut self, key: KeyEvent) {
        let Some(picker) = self.reaction_picker.as_mut() else {
            return;
        };
        match picker.on_key(key) {
            PickerAction::None => {}
            PickerAction::Cancel => self.reaction_picker = None,
            PickerAction::Pick(emoji) => {
                self.reaction_picker = None;
                self.toggle_reaction(emoji).await;
            }
        }
    }

    /// Reacts with `emoji` on the selected message, or removes the reaction if it is ours already
    async fn toggle_reaction(&mut self, emoji: String) -> Option<()> {
        let channel_id = self.selected_channel_id()?;
        let message = self.selected_message()?;
        let user_id = self.signal_manager.user_id();
        let remove = message
            .reactions
            .iter()
            .any(|(id, e)| id == &user_id && e == &emoji);
        self.react(channel_id, emoji, remove).await
    }

    fn start_reply(&mut self) {
        match self.selected_message_id() {
            Some(message_id) => {
                self.replying_to = Some(message_id);
                self.editing = None;
                self.set_focus(Focus::Input);
            }
            None => self.set_status("Select a message to reply to"),
        }
    }

    /// Returns `true` if a reply was cancelled
    fn cancel_reply(&mut self) -> bool {
        let cancelled = self.replying_to.take().is_some();
        if cancelled {
            self.reset_message_selection();
        }
        cancelled
    }

    fn clear_input(&mut self) {
        self.input.take();
        if let Some(channel_id) = self.selected_channel_id() {
            self.pending_attachments.remove(&channel_id);
        }
        self.replying_to = None;
        self.editing = None;
        self.set_status("Message cleared");
    }

    /// Enter on a message: view image, download attachments, or open the link
    fn open_selected_message(&mut self) {
        let Some(message) = self.selected_message().map(Cow::into_owned) else {
            self.set_status("No message selected");
            return;
        };
        if message.attachments.iter().any(|a| is_image(&a.content_type)) {
            open_file(&message);
            self.set_status("Opening image in the default viewer");
        } else if !message.attachments.is_empty() {
            self.save_attachments_to(&message, &downloads_dir());
        } else if open_url(&message, &URL_REGEX).is_some() {
            self.set_status("Opening link in the browser");
        } else if message.attachment_downloads.pending > 0 {
            self.set_status("Attachment is still downloading");
        } else {
            self.set_status("Nothing to open in this message");
        }
    }

    fn save_attachments_to(&mut self, message: &Message, dir: &Path) {
        let mut saved = Vec::new();
        for attachment in &message.attachments {
            match copy_into_dir(&attachment.filename, dir) {
                Ok(dest) => saved.push(dest),
                Err(error) => {
                    error!(%error, src = %attachment.filename.display(), "failed to save");
                    self.set_status(format!("Could not save attachment: {error}"));
                    return;
                }
            }
        }
        match saved.as_slice() {
            [] => self.set_status("This message has no attachments"),
            [one] => self.set_status(format!("Saved to {}", display_path(one))),
            many => self.set_status(format!(
                "Saved {} files to {}",
                many.len(),
                display_path(dir)
            )),
        }
    }

    fn open_save_browser(&mut self) {
        let Some(message) = self.selected_message() else {
            self.set_status("Select a message with an attachment first");
            return;
        };
        if message.attachments.is_empty() {
            self.set_status("This message has no attachments");
            return;
        }
        let Some(message_id) = self.selected_message_id() else {
            return;
        };
        let dir = UiState::load(&self.config.data_dir)
            .last_save_dir
            .unwrap_or_else(downloads_dir);
        self.file_browser = Some(FileBrowser::open(BrowserPurpose::SaveTo(message_id), dir));
    }

    fn open_attach_browser(&mut self) {
        if self.selected_channel_id().is_none() {
            self.set_status("Select a channel first");
            return;
        }
        let dir = UiState::load(&self.config.data_dir)
            .last_attach_dir
            .unwrap_or_else(home_dir);
        self.file_browser = Some(FileBrowser::open(BrowserPurpose::Attach, dir));
    }

    fn on_file_browser_key(&mut self, key: KeyEvent) {
        let Some(browser) = self.file_browser.as_mut() else {
            return;
        };
        let action = browser.on_key(key);
        let purpose = browser.purpose;
        let dir = browser.dir.clone();
        match action {
            BrowserAction::None => {}
            BrowserAction::Cancel => {
                self.file_browser = None;
                if purpose == BrowserPurpose::Attach {
                    self.remember_dir(|state| state.last_attach_dir = Some(dir));
                }
            }
            BrowserAction::Chosen(path) => {
                self.file_browser = None;
                match purpose {
                    BrowserPurpose::Attach => {
                        self.remember_dir(|state| state.last_attach_dir = Some(dir));
                        self.attach_file(path);
                    }
                    BrowserPurpose::SaveTo(message_id) => {
                        self.remember_dir(|state| state.last_save_dir = Some(path.clone()));
                        if let Some(message) = self.storage.message(message_id) {
                            self.save_attachments_to(&message, &path);
                        }
                    }
                }
            }
        }
    }

    fn remember_dir(&self, f: impl FnOnce(&mut UiState)) {
        let mut state = UiState::load(&self.config.data_dir);
        f(&mut state);
        state.save(&self.config.data_dir);
    }

    fn attach_file(&mut self, path: PathBuf) {
        let Some(channel_id) = self.selected_channel_id() else {
            return;
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.pending_attachments
            .entry(channel_id)
            .or_default()
            .push(path);
        self.set_focus(Focus::Input);
        self.set_status(format!("Attached {name}. Press Enter to send"));
    }

    /// Inserts pasted text into the message input (or the channel filter, if shown)
    pub fn paste_text(&mut self, text: &str) {
        if self.select_channel.is_shown {
            text.chars()
                .filter(|c| !c.is_control())
                .for_each(|c| self.select_channel.input.put_char(c));
            return;
        }
        if self.is_popup_shown() {
            return;
        }
        self.set_focus(Focus::Input);
        for c in text.chars() {
            match c {
                '\r' => {}
                '\n' => self.input.new_line(),
                c => self.input.put_char(c),
            }
        }
    }

    fn paste_from_clipboard(&mut self) {
        let Some(clipboard) = self.clipboard.as_mut() else {
            self.set_status("Clipboard is not available");
            return;
        };
        if let Ok(text) = clipboard.get_text()
            && !text.is_empty()
        {
            self.paste_text(&text);
            return;
        }
        match clipboard.get_image() {
            Ok(image) => match encode_png(image) {
                Some((bytes, _, _)) => {
                    let name = format!("clipboard-{}.png", Local::now().format("%Y%m%d-%H%M%S"));
                    let path = std::env::temp_dir().join(name);
                    match std::fs::write(&path, bytes) {
                        Ok(()) => self.attach_file(path),
                        Err(error) => self.set_status(format!("Could not paste image: {error}")),
                    }
                }
                None => self.set_status("Could not read the image from the clipboard"),
            },
            Err(_) => self.set_status("Clipboard is empty"),
        }
    }

    /// Tries to open the first url in the selected message.
    ///
    /// Does nothing if no message is selected and no url is contained in the message.
    fn try_open_url(&mut self) -> Option<()> {
        let message = self.selected_message()?;
        open_url(&message, &URL_REGEX)?;
        self.reset_message_selection();
        Some(())
    }

    /// Tries to open the first file attachment in the selected message.
    ///
    /// Does nothing if no message is selected and the message contains no attachments.
    fn try_open_file(&mut self) -> Option<()> {
        let message = self.selected_message()?;
        open_file(&message)?;
        self.reset_message_selection();
        Some(())
    }

    fn selected_message_id(&self) -> Option<MessageId> {
        let channel_id = self.selected_channel_id()?;
        let pos = self.positions.get(&channel_id)?;
        self.window
            .as_ref()?
            .get(pos.selected?)
            .map(|m| MessageId::new(channel_id, m.arrived_at))
    }

    pub(super) fn selected_message(&self) -> Option<Cow<'_, Message>> {
        let channel_id = self.selected_channel_id()?;
        let pos = self.positions.get(&channel_id)?;
        self.window.as_ref()?.get(pos.selected?).map(Cow::Borrowed)
    }

    /// Returns Some(_) reaction if input is a reaction.
    ///
    /// Inner is None, if the reaction should be removed.
    fn take_reaction(&mut self) -> Option<Option<String>> {
        let input_box = self.get_input();
        if input_box.data.is_empty() {
            Some(None)
        } else {
            let emoji = to_emoji(&input_box.data)?.to_string();
            self.take_input();
            Some(Some(emoji))
        }
    }

    pub async fn add_reaction(
        &mut self,
        channel_id: ChannelId,
        reaction: Option<String>,
    ) -> Option<()> {
        let reaction = reaction.or_else(|| self.take_reaction()?);
        let message = self.selected_message()?;
        let remove = reaction.is_none();
        let emoji = reaction.or_else(|| {
            // find emoji which should be removed
            // if no emoji found => there is no reaction from us => nothing to remove
            message.reactions.iter().find_map(|(id, emoji)| {
                if id == &self.signal_manager.user_id() {
                    Some(emoji.clone())
                } else {
                    None
                }
            })
        })?;
        self.react(channel_id, emoji, remove).await
    }

    /// Sends a reaction on the selected message
    async fn react(&mut self, channel_id: ChannelId, emoji: String, remove: bool) -> Option<()> {
        let channel = self.channel(channel_id)?;
        let message = self.selected_message()?;
        self.signal_manager
            .send_reaction(channel, &message, emoji.clone(), remove);

        let channel_id = channel.id;
        let arrived_at = message.arrived_at;
        self.handle_reaction(
            channel_id,
            arrived_at,
            self.signal_manager.user_id(),
            emoji,
            HandleReactionOptions::new().remove(remove),
        )
        .await;

        self.reset_unread_messages();
        self.bubble_up_channel(channel_id);
        if self.focus != Focus::Messages {
            self.reset_message_selection();
        }

        Some(())
    }

    fn take_input(&mut self) -> String {
        self.get_input().take()
    }

    pub(super) fn send_input(&mut self, channel_id: ChannelId) {
        let input = self.take_input();
        let (input, mut attachments) = Self::extract_attachments(&input, Local::now(), || {
            self.clipboard.as_mut().map(|c| c.get_image())
        });
        for path in self
            .pending_attachments
            .remove(&channel_id)
            .unwrap_or_default()
        {
            match attachment_from_path(&path) {
                Some(attachment) => attachments.push(attachment),
                None => self.set_status(format!("Could not read {}", display_path(&path))),
            }
        }
        let editing = self.editing.take();
        let replying_to = self.replying_to.take();
        let quote = editing
            .is_none()
            .then(|| replying_to.and_then(|id| self.storage.message(id)))
            .flatten();
        let channel = self.channel(channel_id).expect("non-existent channel");
        let (sent_message, response) = self.signal_manager.send_text(
            channel,
            input,
            quote.as_ref(),
            editing.map(|id| id.arrived_at),
            attachments,
        );

        let message_id = MessageId::new(channel_id, sent_message.arrived_at);
        let tx = self.event_tx.clone();
        tokio::spawn(async move {
            if let Ok(result) = response.await {
                tx.send(crate::event::Event::SentTextResult { message_id, result })
                    .expect("event sender gone");
            } else {
                error!(?message_id, "response for sending message was lost");
            }
        });

        if let Some(id) = editing {
            self.store_edited_message(channel_id, id.arrived_at, sent_message);
        } else {
            self.store_message(channel_id, sent_message);
        };

        self.reset_message_selection();
        self.reset_unread_messages();
        self.bubble_up_channel(channel_id);
    }

    pub fn copy_selection(&mut self) {
        if let Some(message) = self.selected_message()
            && let Some(text) = message.message.as_ref()
        {
            let text = text.clone();
            if let Some(clipboard) = self.clipboard.as_mut() {
                if let Err(error) = clipboard.set_text(text) {
                    error!(%error, "failed to copy text to clipboard");
                } else {
                    info!("copied selected text to clipboard");
                }
            }
        }
    }

    /// Returns `true` if editing was reset, otherwise `false`
    fn reset_editing(&mut self) -> bool {
        let is_reset = self.editing.take().is_some();
        if is_reset {
            self.take_input();
        }
        is_reset
    }

    fn start_editing(&mut self) -> Option<()> {
        if !self.input.is_empty() {
            self.set_status("Clear the message input first to edit a message");
            return None;
        }

        let message_id = self.selected_message_id()?;
        let message = self.storage.message(message_id)?;
        if message.from_id != self.user_id {
            self.set_status("You can only edit your own messages");
            return None;
        }

        let target_sent_timestamp = self
            .storage
            .edits(message_id)
            .last()
            .map(|last_edit| last_edit.arrived_at)
            .unwrap_or(message.arrived_at);
        let message_id = MessageId::new(message_id.channel_id, target_sent_timestamp);
        let text = message.message.clone()?;

        self.editing.replace(message_id);
        self.replying_to = None;
        self.input.data = text;
        self.input.on_end();

        Some(())
    }

    fn delete_selected_message(&mut self) -> Option<()> {
        let message_id = self.selected_message_id()?;
        let message = self.storage.message(message_id)?;

        // Only allow deleting own messages
        if message.from_id != self.user_id {
            self.set_status("You can only delete your own messages");
            return None;
        }

        let channel_id = message_id.channel_id;
        let channel = self.storage.channel(channel_id)?;

        if message.deleted || channel_id == ChannelId::User(self.user_id) {
            // Tombstone or Note to Self: fully remove, sync via deleteForMe
            self.signal_manager.send_delete_for_me(&channel, &message);
            self.storage.remove_message(message_id);
            self.remove_message_from_view(channel_id, message.arrived_at);
        } else {
            // Regular channel: delete for everyone (tombstone)
            self.signal_manager
                .send_delete(&channel, message.arrived_at);
            self.storage.delete_message(message_id);
        }

        Some(())
    }

    pub fn event_to_command<'r>(&'r self, event: &KeyEvent) -> Option<&'r Command> {
        let mut combiner = Combiner::default();
        let keys_pressed = combiner.transform(*event)?;
        let modes: &[WindowMode] = if self.is_help() {
            &[WindowMode::Anywhere, WindowMode::Help]
        } else if self.is_select_channel_shown() {
            &[WindowMode::Anywhere, WindowMode::ChannelModal]
        } else {
            match self.focus {
                Focus::Channels => &[WindowMode::Anywhere, WindowMode::Channels, WindowMode::Main],
                Focus::Messages => &[
                    WindowMode::Anywhere,
                    WindowMode::Messages,
                    WindowMode::MessageSelected,
                    WindowMode::Main,
                ],
                Focus::Input => &[WindowMode::Anywhere, WindowMode::Normal, WindowMode::Main],
            }
        };
        for mode in modes {
            if let Some(kb) = self.mode_keybindings.get(mode)
                && let Some(cmd) = kb.get(&keys_pressed)
            {
                return Some(cmd);
            }
        }
        if self.is_help() {
            // Swallow event
            Some(&Command::NoOp)
        } else {
            None
        }
    }

    pub(super) fn extract_attachments<Tz: TimeZone>(
        input: &str,
        at: DateTime<Tz>,
        mut get_clipboard_img: impl FnMut() -> Option<Result<ImageData<'static>, arboard::Error>>,
    ) -> (String, Vec<(AttachmentSpec, Vec<u8>)>)
    where
        Tz::Offset: std::fmt::Display,
    {
        let mut offset = 0;
        let mut clean_input = String::new();

        let attachments = ATTACHMENT_REGEX.find_iter(input).filter_map(|m| {
            let path_str = m.as_str().strip_prefix("file://")?;

            clean_input.push_str(input[offset..m.start()].trim_end());
            offset = m.end();

            if path_str.starts_with("clip") {
                // clipboard
                let img = get_clipboard_img()?
                    .inspect_err(|error| error!(%error, "failed to get clipboard image"))
                    .ok()?;
                let (bytes, width, height) = encode_png(img)?;
                let file_name = format!("screenshot-{}.png", at.format("%Y-%m-%dT%H:%M:%S%z"));

                let spec = AttachmentSpec {
                    content_type: "image/png".to_owned(),
                    length: bytes.len(),
                    file_name: Some(file_name),
                    width: Some(width),
                    height: Some(height),
                    ..Default::default()
                };
                Some((spec, bytes))
            } else {
                // TODO: Show error to user if the file does not exist. This would prevent not
                // sending the attachment in the end.
                attachment_from_path(Path::new(path_str))
            }
        });

        let attachments = attachments.collect();
        clean_input.push_str(&input[offset..]);
        let clean_input = clean_input.trim().to_string();

        (clean_input, attachments)
    }
}

/// Encodes a clipboard image as PNG. Returns the bytes, width and height.
fn encode_png(img: ImageData<'_>) -> Option<(Vec<u8>, u32, u32)> {
    let width: u32 = img.width.try_into().ok()?;
    let height: u32 = img.height.try_into().ok()?;

    let mut bytes = Vec::new();
    let mut cursor = Cursor::new(&mut bytes);
    let encoder = PngEncoder::new(&mut cursor);

    let png: ImageBuffer<Rgba<_>, _> =
        ImageBuffer::from_raw(width, height, img.bytes.into_owned())?;
    let data: Vec<_> = png.into_raw().iter().map(|b| b.swap_bytes()).collect();
    encoder
        .write_image(&data, width, height, image::ExtendedColorType::Rgba8)
        .inspect_err(|error| error!(%error, "failed to encode image"))
        .ok()?;
    Some((bytes, width, height))
}

/// Reads a file to be sent as attachment
fn attachment_from_path(path: &Path) -> Option<(AttachmentSpec, Vec<u8>)> {
    let bytes = std::fs::read(path)
        .inspect_err(|error| error!(path = %path.display(), %error, "failed to read attachment"))
        .ok()?;
    let content_type = mime_guess::from_path(path)
        .first()
        .map(|mime| mime.essence_str().to_string())
        .unwrap_or_default();
    let (width, height) = if is_image(&content_type) {
        image::image_dimensions(path)
            .map(|(w, h)| (Some(w), Some(h)))
            .unwrap_or_default()
    } else {
        Default::default()
    };
    let file_name = path.file_name().map(|f| f.to_string_lossy().into());
    let spec = AttachmentSpec {
        content_type,
        length: bytes.len(),
        file_name,
        width,
        height,
        ..Default::default()
    };
    Some((spec, bytes))
}
