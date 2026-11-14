//! Draw the UI

use std::fmt;

use chrono::Datelike;
use crokey::KeyCombinationFormat;
use itertools::Itertools;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListDirection, ListItem, Paragraph,
};
use ratatui::{Frame, widgets::ListState};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    widgets::Padding,
};
use ratatui::{
    style::{Color, Modifier, Style},
    widgets::Wrap,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use uuid::Uuid;

use crate::app::{App, Focus};
use crate::channels::SelectChannel;
use crate::command::{Command, MessageSelector, WindowMode};
use crate::cursor::Cursor;
use crate::data::{AssociatedValue, Message};
use crate::file_browser::{BrowserPurpose, FileBrowser, display_path, human_size};
use crate::image_preview::{self, is_image};
use crate::reaction_picker::{COLUMNS as PICKER_COLUMNS, ReactionPicker};
use crate::receipt::{Receipt, ReceiptEvent};
use crate::storage::MessageId;
use crate::util::utc_timestamp_msec_to_local;

use super::CHANNEL_VIEW_RATIO;
use super::name_resolver::NameResolver;

/// The main function drawing the UI for each frame
pub fn draw(f: &mut Frame, app: &mut App) {
    if app.is_help() {
        // Display shortcut panel
        let chunks = Layout::default()
            .constraints([
                Constraint::Percentage(5),
                Constraint::Percentage(90),
                Constraint::Percentage(5),
            ])
            .direction(Direction::Horizontal)
            .split(f.area());
        draw_help(f, app, chunks[1]);
        return;
    }

    let chunks = if app.is_channel_list_shown() {
        Layout::default()
            .constraints(
                [
                    Constraint::Ratio(1, CHANNEL_VIEW_RATIO),
                    Constraint::Ratio(3, CHANNEL_VIEW_RATIO),
                ]
                .as_ref(),
            )
            .direction(Direction::Horizontal)
            .split(f.area())
    } else {
        Layout::default()
            .constraints([Constraint::Percentage(100)].as_ref())
            .direction(Direction::Horizontal)
            .split(f.area())
    };

    if app.is_channel_list_shown() {
        draw_sidebar(f, app, chunks[0]);
        draw_chat(f, app, chunks[1]);
    } else {
        draw_chat(f, app, chunks[0]);
    }

    if app.select_channel.is_shown {
        draw_select_channel_popup(f, &mut app.select_channel);
    }
    if let Some(picker) = app.reaction_picker.as_ref() {
        draw_reaction_picker(f, picker);
    }
    if let Some(browser) = app.file_browser.as_mut() {
        draw_file_browser(f, browser);
    }
    if app.search.is_some() {
        draw_search(f, app);
    }
}

/// Block around one of the three main panes, highlighted if focused
fn pane_block<'a>(title: impl Into<Line<'a>>, focused: bool) -> Block<'a> {
    let block = Block::bordered().title(title);
    if focused {
        let style = Style::default().fg(Color::Cyan);
        block
            .border_type(BorderType::Thick)
            .border_style(style)
            .title_style(style.add_modifier(Modifier::BOLD))
    } else {
        block
    }
}

fn popup_block<'a>(title: impl Into<Line<'a>>) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(title)
}

fn hint_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn draw_sidebar(f: &mut Frame, app: &mut App, area: Rect) {
    let (context, hints) = context_hints(app);
    let hints_height = (hints.len() as u16 + 2).min(area.height / 2);
    let chunks = Layout::default()
        .constraints([Constraint::Min(3), Constraint::Length(hints_height)])
        .direction(Direction::Vertical)
        .split(area);
    draw_channels(f, app, chunks[0]);
    draw_hints(f, context, &hints, chunks[1]);
}

fn draw_hints(f: &mut Frame, context: &str, hints: &[(String, &'static str)], area: Rect) {
    let max_key_width = (area.width as usize).saturating_sub(2) / 2;
    let key_width = hints
        .iter()
        .map(|(key, _)| key.width())
        .max()
        .unwrap_or(0)
        .min(max_key_width);
    let key_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let lines: Vec<Line> = hints
        .iter()
        .map(|(key, desc)| {
            let pad = key_width.saturating_sub(key.width());
            Line::from(vec![
                Span::styled(format!("{}{key} ", " ".repeat(pad)), key_style),
                Span::raw(*desc),
            ])
        })
        .collect();
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(hint_style())
        .title(format!(" Keys: {context} "));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// A hint is either bound to a command (the key is looked up in the keybindings) or fixed
enum Hint {
    Cmd(&'static [WindowMode], Command, &'static str),
    Fixed(&'static str, &'static str),
}

const MAIN: &[WindowMode] = &[WindowMode::Main];
const ANYWHERE: &[WindowMode] = &[WindowMode::Anywhere];
const CHANNELS: &[WindowMode] = &[WindowMode::Channels, WindowMode::Main];
const MESSAGES: &[WindowMode] = &[
    WindowMode::Messages,
    WindowMode::MessageSelected,
    WindowMode::Main,
];
const INPUT: &[WindowMode] = &[WindowMode::Normal, WindowMode::Main];

/// Hints for what can be done in the current context
fn context_hints(app: &App) -> (&'static str, Vec<(String, &'static str)>) {
    use Hint::*;
    let (context, hints): (&'static str, Vec<Hint>) = if app.reaction_picker.is_some() {
        (
            "react",
            vec![
                Fixed("←↓↑→ hjkl", "choose emoji"),
                Fixed("Enter", "react / un-react"),
                Fixed("/", "search (last cell)"),
                Fixed("Esc", "cancel"),
            ],
        )
    } else if let Some(browser) = app.file_browser.as_ref() {
        let mut hints = vec![
            Fixed("↑↓ jk", "move"),
            Fixed("Enter", "open / choose"),
            Fixed("h ←", "parent directory"),
            Fixed("/", "filter"),
            Fixed(".", "show hidden files"),
            Fixed("~", "home directory"),
            Fixed("Esc", "cancel"),
        ];
        if matches!(browser.purpose, BrowserPurpose::SaveTo(_)) {
            hints.insert(2, Fixed("s", "save here"));
        }
        ("files", hints)
    } else if app.search.is_some() {
        (
            "find",
            vec![
                Fixed("type", "search text"),
                Fixed("↑↓", "choose result"),
                Fixed("Enter", "go to message"),
                Fixed("Esc", "close"),
            ],
        )
    } else if app.select_channel.is_shown {
        (
            "pick chat",
            vec![
                Fixed("type", "filter by name"),
                Fixed("↑↓", "choose"),
                Fixed("Enter", "open chat"),
                Fixed("Esc", "cancel"),
            ],
        )
    } else {
        match app.focus {
            Focus::Channels => (
                "chats",
                vec![
                    Fixed("↑↓ jk", "choose chat"),
                    Cmd(CHANNELS, Command::FocusInput, "write message"),
                    Cmd(MAIN, Command::FocusNext, "next pane"),
                    Cmd(MAIN, Command::NewMessage, "new chat"),
                    Cmd(MAIN, Command::ToggleChannelModal, "jump to chat"),
                    Cmd(MAIN, Command::ToggleMuteChannel, "mute / unmute"),
                    Cmd(MAIN, Command::Search, "find in chat"),
                    Cmd(ANYWHERE, Command::Help, "all shortcuts"),
                    Cmd(ANYWHERE, Command::Quit, "quit"),
                ],
            ),
            Focus::Messages => (
                "messages",
                vec![
                    Fixed("↑↓ jk", "select message"),
                    Cmd(MESSAGES, Command::OpenMessage, "open / download"),
                    Cmd(MESSAGES, Command::ReactPicker, "react"),
                    Cmd(MESSAGES, Command::Reply, "reply"),
                    Cmd(MESSAGES, Command::EditMessage, "edit (own)"),
                    Cmd(MESSAGES, Command::DeleteMessage, "delete (own)"),
                    Cmd(
                        MESSAGES,
                        Command::CopyMessage(MessageSelector::Selected),
                        "copy text",
                    ),
                    Cmd(MESSAGES, Command::SaveAttachment, "save to folder"),
                    Cmd(MESSAGES, Command::AttachFile, "attach file"),
                    Cmd(MAIN, Command::Search, "find in chat"),
                    Fixed("Esc", "back to input"),
                ],
            ),
            Focus::Input => (
                "message",
                vec![
                    Fixed("Enter", "send"),
                    Cmd(INPUT, Command::NewLine, "new line"),
                    Cmd(INPUT, Command::AttachFile, "attach file"),
                    Cmd(MAIN, Command::Paste, "paste"),
                    Cmd(INPUT, Command::ClearInput, "clear"),
                    Cmd(INPUT, Command::OpenEditor, "external editor"),
                    Fixed(":name:", "emoji (Tab completes)"),
                    Fixed("Esc", "cancel reply / edit"),
                    Cmd(MAIN, Command::FocusNext, "next pane"),
                ],
            ),
        }
    };
    let hints = hints
        .into_iter()
        .filter_map(|hint| match hint {
            Cmd(modes, cmd, desc) => Some((keys_for(app, modes, &cmd)?, desc)),
            Fixed(key, desc) => Some((key.to_string(), desc)),
        })
        .collect();
    (context, hints)
}

/// Formats the (at most two) keys bound to `cmd` in `modes`, e.g. "Ctrl+M/Alt+M"
fn keys_for(app: &App, modes: &[WindowMode], cmd: &Command) -> Option<String> {
    let format = KeyCombinationFormat::default()
        .with_control("Ctrl+")
        .with_alt("Alt+")
        .with_shift("Shift+");
    let mut keys: Vec<String> = modes
        .iter()
        .filter_map(|mode| app.mode_keybindings.get(mode))
        .flat_map(|kb| kb.iter().filter(|(_, c)| *c == cmd).map(|(k, _)| *k))
        .map(|k| {
            let key = format.to_string(k);
            // "Ctrl+r" => "Ctrl+R"
            match key.rsplit_once('+') {
                Some((mods, c)) if c.len() == 1 => format!("{mods}+{}", c.to_uppercase()),
                _ => key,
            }
        })
        .collect();
    // prefer named keys and combinations over plain letters, ctrl over alt
    keys.sort_by_key(|k| (k.len() == 1, k.contains("Alt+"), k.len(), k.clone()));
    keys.dedup();
    if keys.is_empty() {
        None
    } else {
        Some(keys.into_iter().take(2).join("/"))
    }
}

fn draw_select_channel_popup(f: &mut Frame, select_channel: &mut SelectChannel) {
    let area = centered_rect(60, 60, f.area());
    let chunks = Layout::default()
        .constraints([Constraint::Length(1 + 2), Constraint::Min(0)].as_ref())
        .direction(Direction::Vertical)
        .split(area);
    f.render_widget(Clear, area);
    let title = if select_channel.title.is_empty() {
        "Select channel"
    } else {
        select_channel.title
    };
    let input = Paragraph::new(Text::from(select_channel.input.data.clone()))
        .block(popup_block(format!(" {title} ")));
    f.render_widget(input, chunks[0]);
    let cursor = &select_channel.input.cursor;
    f.set_cursor_position((
        chunks[0].x + cursor.col as u16 + 1,
        chunks[0].y + cursor.line as u16 + 1,
    ));
    let items: Vec<_> = select_channel.filtered_names().map(ListItem::new).collect();
    match select_channel.state.selected() {
        Some(idx) if items.len() <= idx => {
            select_channel.state.select(items.len().checked_sub(1));
        }
        None if !items.is_empty() => {
            select_channel.state.select(Some(0));
        }
        _ => (),
    }
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL))
        .highlight_style(Style::default().reversed());
    f.render_stateful_widget(list, chunks[1], &mut select_channel.state);
}

fn draw_channels(f: &mut Frame, app: &mut App, area: Rect) {
    let channel_list_width = area.width.saturating_sub(2) as usize;
    let channels = app.channels().iter().map(|channel| {
        let unread_messages_label = if channel.unread_messages != 0 {
            format!(" ({})", channel.unread_messages)
        } else {
            String::new()
        };
        let mute_label = if channel.muted { " [M]" } else { "" };
        let typing_label = if channel.is_writing() { " [T]" } else { "" };
        let suffix = format!("{unread_messages_label}{mute_label}{typing_label}");
        let channel_name = app.channel_name(channel);
        let label = format!("{channel_name}{suffix}");
        let label_width = label.width();
        let label = if label_width <= channel_list_width || suffix.is_empty() {
            label
        } else {
            let diff = label_width - channel_list_width;
            let mut end = channel.name.width().saturating_sub(diff);
            while !channel.name.is_char_boundary(end) {
                end += 1;
            }
            format!("{}{}", &channel.name[0..end], suffix)
        };
        ListItem::new(label)
    });

    let focused = app.focus == Focus::Channels && !app.is_popup_shown();
    let highlight_style = if focused {
        Style::default().reversed()
    } else {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    };
    let channels = List::new(channels)
        .block(pane_block(" Chats ", focused))
        .highlight_style(highlight_style);
    let no_channels = channels.is_empty();
    f.render_stateful_widget(channels, area, &mut app.channels.state);
    app.channel_list_area.set(area);

    if no_channels {
        f.render_widget(
            Paragraph::new("No channels\n(Channels will be added on incoming messages)")
                .wrap(Wrap { trim: true })
                .centered()
                .block(Block::default().padding(Padding::top(area.height / 2))),
            area,
        );
    };
}

fn wrap(text: &str, mut cursor: Cursor, width: usize) -> (String, Cursor, usize) {
    let mut res = String::new();

    let mut line = 0;
    let mut col = 0;

    for c in text.chars() {
        // current line too long => wrap
        if col > 0 && col % width == 0 {
            res.push('\n');

            // adjust cursor
            if line < cursor.line {
                cursor.line += 1;
                cursor.idx += 1;
            } else if line == cursor.line && col <= cursor.col {
                cursor.line += 1;
                cursor.col -= col;
                cursor.idx += 1;
            }

            line += 1;
            col = 0;
        }

        if c == '\n' {
            line += 1;
            col = 0;
        } else {
            col += c.width().unwrap_or(0);
        }
        res.push(c);
    }

    // special case: cursor is at the end of the text and overflows `width`
    if cursor.idx == res.len() && cursor.col == width {
        res.push('\n');
        cursor.line += 1;
        cursor.col = 0;
        cursor.idx += 1;
        line += 1;
    }

    (res, cursor, line + 1)
}

fn draw_chat(f: &mut Frame, app: &mut App, area: Rect) {
    let text_width = area.width.saturating_sub(2) as usize;
    let (wrapped_input, cursor, num_input_lines) =
        wrap(&app.input.data, app.input.cursor.clone(), text_width);

    let chunks = Layout::default()
        .constraints(
            [
                Constraint::Min(0),
                Constraint::Length(num_input_lines as u16 + 2),
            ]
            .as_ref(),
        )
        .direction(Direction::Vertical)
        .split(area);

    draw_messages(f, app, chunks[0]);

    let focused = app.focus == Focus::Input && !app.is_popup_shown();
    let mut title = vec![Span::raw(" Message ")];
    if app.is_editing() {
        title.push(Span::styled(
            "· editing (Esc cancels) ",
            Style::default().fg(Color::Yellow),
        ));
    } else if let Some(message) = app.replying_to().and_then(|id| app.storage.message(id)) {
        let name = app.name_by_id_cached(message.from_id);
        let snippet: String = message
            .message
            .as_deref()
            .unwrap_or("attachment")
            .replace('\n', " ")
            .chars()
            .take(30)
            .collect();
        title.push(Span::styled(
            format!("· reply to {name}: {snippet} (Esc cancels) "),
            Style::default().fg(Color::Yellow),
        ));
    }
    let attachments = app.pending_attachments();
    if !attachments.is_empty() {
        let names = attachments
            .iter()
            .filter_map(|p| p.file_name())
            .map(|n| n.to_string_lossy())
            .join(", ");
        title.push(Span::styled(
            format!("· 📎 {names} "),
            Style::default().fg(Color::Green),
        ));
    }
    let mut block = pane_block(Line::from(title), focused);
    if let Some(status) = app.status() {
        block = block.title_bottom(
            Line::styled(format!(" {status} "), Style::default().fg(Color::Yellow))
                .right_aligned(),
        );
    }

    let input = if app.input.data.is_empty() {
        let placeholder = if focused {
            "Type a message, Enter to send"
        } else {
            "Press Tab to move here and type a message"
        };
        Paragraph::new(Span::styled(placeholder, hint_style()))
    } else {
        Paragraph::new(Text::from(wrapped_input))
    };
    f.render_widget(input.block(block), chunks[1]);
    if focused {
        f.set_cursor_position((
            chunks[1].x + cursor.col as u16 + 1,  // +1 for frame
            chunks[1].y + cursor.line as u16 + 1, // +1 for frame
        ));
    }
}

// TODO: This does IO and needs to be moved into App
fn prepare_receipts(app: &mut App) {
    let user_id = app.user_id;
    let Some(window) = app.window.as_ref() else {
        return;
    };
    let channel_id = window.channel_id();
    let viewport_bottom = app
        .positions
        .get(&channel_id)
        .and_then(|pos| pos.viewport_bottom);

    // viewport_bottom and everything older that's loaded
    let read_messages: Vec<Message> = window
        .iter_rev_from(viewport_bottom)
        .filter(|msg| matches!(msg.receipt, Receipt::Delivered) && msg.from_id != user_id)
        .map(|msg| {
            let mut msg = msg.clone();
            msg.receipt = Receipt::Read;
            msg
        })
        .collect();

    for message in read_messages {
        let from_id = message.from_id;
        let arrived_at = message.arrived_at;
        app.store_message(channel_id, message);
        app.add_receipt_event(ReceiptEvent::new(from_id, arrived_at, Receipt::Read));
    }
}

fn draw_messages(f: &mut Frame, app: &mut App, area: Rect) {
    // area without borders
    let height = area.height.saturating_sub(2) as usize;
    let width = area.width.saturating_sub(2) as usize;
    if height == 0 || width == 0 {
        return;
    }
    app.message_view_height = height;

    // to be removed because we don't want any IO in the rendering pass
    prepare_receipts(app);

    let focused = app.focus == Focus::Messages && !app.is_popup_shown();
    let title = match app.selected_channel() {
        Some(channel) => {
            let muted = if channel.muted { " (muted)" } else { "" };
            format!(" {}{muted} ", app.channel_name(channel))
        }
        None => " Messages ".to_string(),
    };

    let Some(window) = app.window.as_ref() else {
        // No channel selected
        f.render_widget(
            Paragraph::new("No chat selected")
                .block(pane_block(title, focused).padding(Padding::top(area.height / 2)))
                .centered(),
            area,
        );
        return;
    };

    if window.is_empty() {
        // No messages
        f.render_widget(
            Paragraph::new("No messages yet")
                .block(pane_block(title, focused).padding(Padding::top(area.height / 2)))
                .centered(),
            area,
        );
        return;
    }

    let pos = app
        .positions
        .entry(window.channel_id())
        .or_default()
        .clone();
    let selected = pos.selected;

    // calculate the bottom anchor
    let mut bottom = pos
        .viewport_bottom
        .unwrap_or_else(|| window.newest().expect("logic error: no messages"));
    if let Some(selected) = pos.selected
        && bottom < selected
    {
        // selected is newer than bottom => pull it to the bottom edge
        bottom = selected;
    }

    // build candidates and fit them into the viewport
    let (items, ats, heights) = build_candidates(app, bottom, width, height, pos.selected);
    let selected_pos = selected.and_then(|s| ats.iter().position(|&at| at == s));
    let (start, end) = fit_window(&heights, height, selected_pos);

    // the selected message is highlighted while building the items (to keep image colors)
    let visible = items[start..end].to_vec();
    let list = List::new(visible)
        .block(pane_block(title, focused))
        .direction(ListDirection::BottomToTop);
    let mut state = ListState::default();
    state.select(selected_pos.map(|s| s - start));
    f.render_stateful_widget(list, area, &mut state);

    // update the bottom anchor but only when we are not sticking to the bottom
    let new_bottom = ats[start];
    let at_tail = window.at_newest() && window.newest() == Some(new_bottom);
    let channel_id = window.channel_id();
    app.positions.entry(channel_id).or_default().viewport_bottom = (!at_tail).then_some(new_bottom);
}

fn build_candidates(
    app: &App,
    bottom: u64,
    width: usize,
    height: usize,
    until: Option<u64>,
) -> (Vec<ListItem<'static>>, Vec<u64>, Vec<usize>) {
    let Some(window) = app.window.as_ref() else {
        return Default::default();
    };
    let channel_id = window.channel_id();

    let unread_messages = app
        .channel(channel_id)
        .map(|c| c.unread_messages as usize)
        .unwrap_or(0);

    // collect messages >= height and far enough to include `until`
    let mut candidates: Vec<&Message> = Vec::new();
    let mut seen_until = until.is_none();
    for message in window.iter_rev_from(Some(bottom)) {
        if Some(message.arrived_at) == until {
            seen_until = true;
        }
        candidates.push(message);
        if candidates.len() >= height && seen_until {
            break;
        }
    }

    // precalculate names for candidates
    let names = NameResolver::compute(
        app,
        candidates
            .iter()
            .map(|message| MessageId::new(channel_id, message.arrived_at)),
    );

    // message display options
    const TIME_WIDTH: usize = 6; // width of "00:00 "
    let mut prefix_width = TIME_WIDTH;
    if app.config.show_receipts {
        prefix_width += RECEIPT_WIDTH;
    }
    let prefix = " ".repeat(prefix_width);

    // Build items bottom-first, parallel arrived_at and height
    let mut items: Vec<ListItem<'static>> = Vec::new();
    let mut items_ats: Vec<u64> = Vec::new();
    let mut heights: Vec<usize> = Vec::new();

    let mut candidates = candidates.iter().enumerate().peekable();
    while let Some((idx, msg)) = candidates.next() {
        // the iterator is reversed, so the peeked item is the older, *previous* message
        let prev_msg = candidates.peek();
        let date_division = display_date_line(
            msg.arrived_at,
            prev_msg.map(|(_, msg)| msg.arrived_at),
            width,
        );

        let new_messages_division =
            (unread_messages > 0 && unread_messages == idx + 1).then(|| {
                "-".repeat(prefix_width)
                    + "new messages"
                    + &"-".repeat(width.saturating_sub(prefix_width))
            });
        let show_receipt = ShowReceipt::from_msg(msg, app.user_id, app.config.show_receipts);

        if let Some(mut lines) = display_message_lines(
            &names,
            msg,
            &prefix,
            width,
            show_receipt,
            date_division,
            new_messages_division,
            app.config.colored_messages,
        ) {
            if until == Some(msg.arrived_at) {
                for line in &mut lines {
                    line.style = line.style.patch(Style::default().reversed());
                }
            }
            lines.extend(image_previews(app, msg, &prefix, width, height));
            truncate_lines(&mut lines, height, &prefix);
            let item = ListItem::new(Text::from(lines));
            let h = item.height();
            items.push(item);
            items_ats.push(msg.arrived_at);
            heights.push(h);
        }
    }

    (items, items_ats, heights)
}

/// Colored previews of the image attachments of a message
fn image_previews(
    app: &App,
    msg: &Message,
    prefix: &str,
    width: usize,
    height: usize,
) -> Vec<Line<'static>> {
    if msg.deleted {
        return Vec::new();
    }
    const MAX_COLS: usize = 48;
    const MAX_ROWS: usize = 12;
    let max_cols = width.saturating_sub(prefix.len()).min(MAX_COLS);
    let max_rows = (height / 2).min(MAX_ROWS);
    msg.attachments
        .iter()
        .filter(|a| is_image(&a.content_type))
        .filter_map(|a| app.image_preview(&a.filename))
        .flat_map(|thumbnail| image_preview::render(&thumbnail, max_cols, max_rows, prefix))
        .collect()
}

/// Limits the lines of a message to `height`
fn truncate_lines(lines: &mut Vec<Line<'static>>, height: usize, prefix: &str) {
    if lines.len() > height {
        // message is too big to be shown fully
        lines.truncate(height.saturating_sub(1));
        lines.push(Line::from(format!("{prefix}[...]")));
    }
}

fn fit_window(heights: &[usize], height: usize, selected: Option<usize>) -> (usize, usize) {
    if heights.is_empty() {
        return (0, 0);
    }

    let mut rows = 0;
    let mut end = 0;
    while end < heights.len() && rows + heights[end] <= height {
        rows += heights[end];
        end += 1;
    }
    match selected {
        Some(selected) if end <= selected => {
            // selection is above the fold -> pin it to the top, fit downward
            let mut rows = 0;
            let mut start = selected;
            let mut i = selected;
            loop {
                if height < rows + heights[i] && i != selected {
                    break;
                }
                rows += heights[i];
                start = i;
                if i == 0 {
                    break;
                }
                i -= 1;
            }
            (start, selected.saturating_add(1))
        }
        _ => (0, end),
    }
}

fn display_time(timestamp: u64) -> String {
    utc_timestamp_msec_to_local(timestamp)
        .format("%R ")
        .to_string()
}

const RECEIPT_WIDTH: usize = 2;

/// Ternary state whether to show receipt for a message
enum ShowReceipt {
    // show receipt for this message
    Yes,
    // don't show receipt for this message
    No,
    // don't show receipt for any message
    Never,
}

impl ShowReceipt {
    fn from_msg(msg: &Message, user_id: Uuid, config_show_receipts: bool) -> Self {
        if config_show_receipts {
            if user_id == msg.from_id {
                Self::Yes
            } else {
                Self::No
            }
        } else {
            Self::Never
        }
    }
}

fn display_receipt(receipt: Receipt, show: ShowReceipt) -> &'static str {
    use ShowReceipt::*;
    match (show, receipt) {
        (Yes, Receipt::Nothing) => "  ",
        (Yes, Receipt::Sent) => "○ ",
        (Yes, Receipt::Delivered) => "◉ ",
        (Yes, Receipt::Read) => "● ",
        (No, _) => "  ",
        (Never, _) => "",
    }
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn display_message(
    names: &NameResolver,
    msg: &Message,
    prefix: &str,
    width: usize,
    height: usize,
    show_receipt: ShowReceipt,
    date_division: Option<String>,
    unread_messages_division: Option<String>,
    colored_messages: bool,
) -> Option<ListItem<'static>> {
    let mut lines = display_message_lines(
        names,
        msg,
        prefix,
        width,
        show_receipt,
        date_division,
        unread_messages_division,
        colored_messages,
    )?;
    truncate_lines(&mut lines, height, prefix);
    Some(ListItem::new(Text::from(lines)))
}

#[allow(clippy::too_many_arguments)]
fn display_message_lines(
    names: &NameResolver,
    msg: &Message,
    prefix: &str,
    width: usize,
    show_receipt: ShowReceipt,
    date_division: Option<String>,
    unread_messages_division: Option<String>,
    colored_messages: bool,
) -> Option<Vec<Line<'static>>> {
    let receipt = Span::styled(
        display_receipt(msg.receipt, show_receipt),
        Style::default().fg(Color::Yellow),
    );

    let time = Span::styled(
        display_time(msg.arrived_at),
        Style::default().fg(Color::Yellow),
    );

    let (from, from_color) = names.resolve(msg.from_id);

    let from_width = from.width();
    let from = Span::styled(from.into_owned(), Style::default().fg(from_color));
    let delimiter = Span::from(": ");

    // collect message text
    let mut text = if msg.deleted {
        String::from("[message deleted]")
    } else {
        let text = strip_ansi_escapes::strip_str(msg.message.as_deref().unwrap_or_default());
        let mut text = replace_mentions(msg, names, text);
        add_attachments(msg, &mut text);
        if text.is_empty() {
            return None; // no text => nothing to render
        }
        add_reactions(msg, &mut text);
        add_edited(msg, &mut text);
        text
    };
    add_expire_countdown(msg, &mut text);

    let mut spans: Vec<Line> = vec![];
    if let Some(date_division) = date_division {
        spans.push(Line::from(date_division));
    }
    if let Some(unread_messages_division) = unread_messages_division {
        spans.push(Line::from(unread_messages_division));
    }

    // prepend quote if any
    let quote_text = msg
        .quote
        .as_ref()
        .and_then(|quote| displayed_quote(names, quote));
    if let Some(quote_text) = quote_text.as_ref() {
        let quote_prefix = format!("{prefix}> ");
        let quote_wrap_opts = textwrap::Options::new(width.saturating_sub(2))
            .initial_indent(&quote_prefix)
            .subsequent_indent(&quote_prefix);
        let quote_style = Style::default().fg(Color::Rgb(150, 150, 150));
        spans.extend(
            textwrap::wrap(quote_text, quote_wrap_opts)
                .into_iter()
                .enumerate()
                .map(|(idx, line)| {
                    let res = if idx == 0 {
                        vec![
                            receipt.clone(),
                            time.clone(),
                            from.clone(),
                            delimiter.clone(),
                            Span::styled(
                                line.strip_prefix(prefix).unwrap().to_owned(),
                                quote_style,
                            ),
                        ]
                    } else {
                        vec![Span::styled(line.into_owned(), quote_style)]
                    };
                    Line::from(res)
                }),
        );
    }

    let add_time = quote_text.is_none();
    let message_style = if msg.deleted {
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC)
    } else if colored_messages {
        Style::default().fg(from_color)
    } else {
        Style::default()
    };

    const DELIMITER_WIDTH: usize = 2;
    let first_line_prefix = " ".repeat(prefix.len() + from_width + DELIMITER_WIDTH);
    let wrap_opts = textwrap::Options::new(width)
        .initial_indent(if add_time { &first_line_prefix } else { prefix })
        .subsequent_indent(prefix);
    let mut wrapped_text = textwrap::wrap(&text, &wrap_opts).into_iter();

    if add_time && let Some(first_line) = wrapped_text.next() {
        let line = first_line
            .strip_prefix(&first_line_prefix)
            .expect("logic error")
            .to_owned();
        spans.push(Line::from(vec![
            receipt,
            time,
            from,
            delimiter,
            Span::styled(line, message_style),
        ]));
    }
    spans.extend(
        wrapped_text.map(|line| Line::from(Span::styled(line.into_owned(), message_style))),
    );

    if let Some(reason) = msg.send_failed.as_deref() {
        let error = format!("[Could not send: {reason}]");
        let error_style = Style::default().fg(Color::Red);
        spans.extend(
            textwrap::wrap(&error, &wrap_opts)
                .into_iter()
                .map(|line| Span::styled(line.into_owned(), error_style).into()),
        );
    }

    Some(spans)
}

fn replace_mentions(msg: &Message, names: &NameResolver, text: String) -> String {
    if msg.body_ranges.is_empty() {
        return text;
    }

    let ac = aho_corasick::AhoCorasickBuilder::new()
        .build(std::iter::repeat_n("￼", msg.body_ranges.len())) // TODO: cache
        .expect("failed to build obj replacer");
    let mut buf = String::with_capacity(text.len());
    let mut ranges = msg.body_ranges.iter();
    ac.replace_all_with(&text, &mut buf, |_, _, dst| {
        // TODO: check ranges?
        for range in &mut ranges {
            let (name, _color) = match range.value {
                AssociatedValue::MentionUuid(id) => names.resolve(id),
                AssociatedValue::Style(_) => continue, // not supported yet
            };
            dst.push('@');
            dst.push_str(&name);
            return true;
        }
        false
    });

    buf
}

fn display_date_line(
    msg_timestamp: u64,
    previous_msg_timestamp: Option<u64>,
    width: usize,
) -> Option<String> {
    let local_time = utc_timestamp_msec_to_local(msg_timestamp);
    let current_msg_day = local_time.num_days_from_ce();
    let previous_msg_day =
        previous_msg_timestamp.map(|t| utc_timestamp_msec_to_local(t).num_days_from_ce());

    if Some(current_msg_day) != previous_msg_day {
        let date = format!("{:=^width$}", local_time.format(" %A, %x "));
        Some(date)
    } else {
        None
    }
}

fn add_attachments(msg: &Message, out: &mut String) {
    for attachment in &msg.attachments {
        if !out.is_empty() {
            out.push('\n');
        }
        let kind = if attachment.content_type.starts_with("audio/") {
            "voice note"
        } else if is_image(&attachment.content_type) {
            "image"
        } else {
            "file"
        };
        let name = attachment
            .filename
            .file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_default();
        out.push_str(&format!("[{kind}: {name}]"));
    }
    if msg.attachment_downloads.failed > 0 {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!(
            "<failed to download attachments ({})>",
            msg.attachment_downloads.failed
        ));
    }
    if msg.attachment_downloads.pending > 0 {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!(
            "<downloading attachments ({})...>",
            msg.attachment_downloads.pending
        ));
    }
}

fn add_reactions(msg: &Message, out: &mut dyn fmt::Write) {
    if !msg.reactions.is_empty() {
        fmt::write(
            out,
            format_args!(
                " [{}]",
                msg.reactions.iter().map(|(_, emoji)| emoji).format("")
            ),
        )
        .expect("formatting reactions failed");
    }
}

fn add_edited(msg: &Message, out: &mut dyn fmt::Write) {
    if msg.edited {
        write!(out, " [edited]").expect("formatting edited failed")
    }
}

fn format_duration_short(seconds: u64) -> String {
    if seconds >= 604800 {
        format!("{}w", seconds / 604800)
    } else if seconds >= 86400 {
        format!("{}d", seconds / 86400)
    } else if seconds >= 3600 {
        format!("{}h", seconds / 3600)
    } else if seconds >= 60 {
        format!("{}m", seconds / 60)
    } else {
        format!("{seconds}s")
    }
}

fn add_expire_countdown(msg: &Message, out: &mut dyn fmt::Write) {
    if msg.expire_timer.is_some_and(|t| t > 0) {
        if let Some(expires_at) = msg.expires_at {
            let now_ms = crate::util::utc_now_timestamp_msec();
            if now_ms < expires_at {
                let remaining_secs = (expires_at - now_ms) / 1000;
                write!(out, " [{}]", format_duration_short(remaining_secs))
            } else {
                write!(out, " [expired]")
            }
            .expect("formatting countdown failed");
        } else {
            write!(out, " [pending]").expect("formatting countdown failed");
        }
    }
}

fn help_commands<'a>() -> Vec<Line<'a>> {
    let commands = <Command as strum::IntoEnumIterator>::iter()
        .map(|cmd| {
            (
                strum::EnumProperty::get_str(&cmd, "usage")
                    .unwrap_or(&cmd.to_string())
                    .to_string(),
                strum::EnumProperty::get_str(&cmd, "desc")
                    .unwrap_or("Undocumented")
                    .to_string(),
            )
        })
        .collect_vec();
    let usage_len = commands.iter().map(|inf| inf.0.len()).max().unwrap_or(0);
    let commands = commands
        .iter()
        .map(|inf| Line::raw(format!("{: <usage_len$}   {}", inf.0, inf.1)));
    let mut v = vec![
        Line::styled("Commands", Style::default().add_modifier(Modifier::BOLD)),
        Line::default(),
    ];
    v.extend(commands);
    v
}

fn bindings(app: &App) -> Vec<Line<'_>> {
    [
        WindowMode::Main,
        WindowMode::Channels,
        WindowMode::Messages,
        WindowMode::Normal,
        WindowMode::Anywhere,
        WindowMode::Help,
        WindowMode::ChannelModal,
        WindowMode::MessageSelected,
    ]
    .iter()
    .filter(|mode| app.mode_keybindings.get(mode).is_some_and(|kb| !kb.is_empty()))
    .map(|mode| bindings_mode(app, mode))
    .concat()
}

fn help_quick_guide<'a>() -> Vec<Line<'a>> {
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let mut v = vec![
        Line::styled("Quick guide", bold),
        Line::default(),
    ];
    v.extend(
        [
            "The screen has three panes: chats (left), messages and the message input.",
            "Tab / Shift+Tab moves between them; the focused pane has a thick cyan border.",
            "The box at the bottom left always lists what you can do in the current pane.",
            "",
            "Chats:     up/down or j/k to switch chat, Enter to start typing.",
            "Messages:  up/down or j/k to select a message, then:",
            "           Enter opens an image / downloads a file to ~/Downloads / opens a link,",
            "           Ctrl+R reacts, r replies, e edits, Ctrl+D deletes, y copies,",
            "           Ctrl+S saves attachments to a folder of your choice.",
            "Input:     Enter sends, Alt+Enter (or Shift+Enter) adds a new line.",
            "Anywhere:  Ctrl+U attach file, Ctrl+V paste, Ctrl+F find, Ctrl+N new chat,",
            "           Ctrl+M (or Alt+M) mute chat, Ctrl+P jump to chat, Ctrl+C quit.",
            "",
            "Some terminals cannot distinguish Ctrl+M from Enter or Ctrl+Shift+U from Ctrl+U;",
            "use Alt+M and Alt+U there.",
            "",
        ]
        .into_iter()
        .map(Line::raw),
    );
    v
}

fn bindings_mode<'a>(app: &App, mode: &WindowMode) -> Vec<Line<'a>> {
    let bindings = if let Some(kb) = app.mode_keybindings.get(mode) {
        kb.iter()
            .map(|(kc, cmd)| {
                (
                    kc.to_string(),
                    cmd.to_string(),
                    strum::EnumProperty::get_str(cmd, "desc")
                        .unwrap_or("Undocumented")
                        .to_string(),
                )
            })
            .sorted()
            .collect_vec()
    } else {
        Vec::default()
    };
    let kc_len = bindings.iter().map(|inf| inf.0.len()).max().unwrap_or(0);
    let cmd_len = bindings.iter().map(|inf| inf.1.len()).max().unwrap_or(0);
    let bindings = bindings.iter().map(|inf| {
        Line::raw(format!(
            "{: <kc_len$}  {: <cmd_len$}  {}",
            inf.0, inf.1, inf.2
        ))
    });
    let mut v = vec![
        Line::default(),
        Line::styled(
            format!("Bindings for {} (config section [{mode}])", mode_title(mode)),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::default(),
    ];
    v.extend(bindings);
    v
}

fn mode_title(mode: &WindowMode) -> &'static str {
    match mode {
        WindowMode::Anywhere => "everywhere",
        WindowMode::Main => "all panes",
        WindowMode::Channels => "the chats pane",
        WindowMode::Messages | WindowMode::MessageSelected => "the messages pane",
        WindowMode::Normal => "the message input",
        WindowMode::Help => "this help",
        WindowMode::ChannelModal => "the chat picker",
        WindowMode::Multiline => "multi-line input (unused)",
    }
}

fn help_indicators<'a>() -> Vec<Line<'a>> {
    let indicators: &[(&str, &str)] = &[
        ("(N)", "N unread messages in channel"),
        ("[M]", "Channel is muted (notifications silenced)"),
        ("○", "Message sent"),
        ("◉", "Message delivered"),
        ("●", "Message read"),
    ];
    let label_len = indicators.iter().map(|i| i.0.len()).max().unwrap_or(0);
    let mut v = vec![
        Line::default(),
        Line::styled("Indicators", Style::default().add_modifier(Modifier::BOLD)),
        Line::default(),
    ];
    v.extend(
        indicators
            .iter()
            .map(|i| Line::raw(format!("{: <label_len$}   {}", i.0, i.1))),
    );
    v
}

fn draw_help(f: &mut Frame, app: &mut App, area: Rect) {
    let mut command_bindings = help_quick_guide();
    command_bindings.extend(help_indicators());
    command_bindings.extend(bindings(app));
    command_bindings.push(Line::default());
    command_bindings.extend(help_commands());
    let command_bindings = Paragraph::new(Text::from(command_bindings))
        .block(popup_block(" Help: shortcuts (Esc to close, ↑↓ to scroll) "))
        .scroll(app.help_scroll);
    f.render_widget(command_bindings, area);
}

fn displayed_quote(names: &NameResolver, quote: &Message) -> Option<String> {
    let (name, _) = names.resolve(quote.from_id);
    let text = format!("({}) {}", name, quote.message.as_ref()?);
    Some(replace_mentions(quote, names, text))
}

fn fixed_centered_rect(width: u16, height: u16, r: Rect) -> Rect {
    let width = width.min(r.width);
    let height = height.min(r.height);
    Rect {
        x: r.x + (r.width - width) / 2,
        y: r.y + (r.height - height) / 2,
        width,
        height,
    }
}

const PICKER_CELL_WIDTH: usize = 5;

fn draw_reaction_picker(f: &mut Frame, picker: &ReactionPicker) {
    let items = picker.items();
    let grid_rows = items.len().div_ceil(PICKER_COLUMNS);
    let width = (PICKER_COLUMNS * PICKER_CELL_WIDTH) as u16 + 4;
    let height = grid_rows as u16 + 6;
    let area = fixed_centered_rect(width, height, f.area());
    f.render_widget(Clear, area);

    let selected_style = Style::default()
        .bg(Color::Cyan)
        .fg(Color::Black)
        .add_modifier(Modifier::BOLD);
    let mut lines: Vec<Line> = vec![Line::default()];
    for (row, chunk) in items.chunks(PICKER_COLUMNS).enumerate() {
        let mut spans = vec![Span::raw(" ")];
        for (col, emoji) in chunk.iter().enumerate() {
            let idx = row * PICKER_COLUMNS + col;
            let pad = PICKER_CELL_WIDTH.saturating_sub(emoji.width() + 1);
            let cell = format!(" {emoji}{}", " ".repeat(pad));
            let style = if idx == picker.selected {
                selected_style
            } else {
                Style::default()
            };
            spans.push(Span::styled(cell, style));
        }
        lines.push(Line::from(spans));
    }
    if items.is_empty() {
        lines.push(Line::styled("  no matching emoji", hint_style()));
    }
    lines.push(Line::default());
    let search_text = if picker.query.is_empty() && !picker.is_search_selected() {
        "🔎 more… (type to search)".to_string()
    } else {
        format!("🔎 {}▏", picker.query)
    };
    let search_style = if picker.is_search_selected() {
        selected_style
    } else {
        hint_style()
    };
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(search_text, search_style),
    ]));

    let block = popup_block(" React ").title_bottom(
        Line::styled(" Enter: react · Esc: cancel ", hint_style()).centered(),
    );
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_file_browser(f: &mut Frame, browser: &mut FileBrowser) {
    let area = centered_rect(70, 70, f.area());
    f.render_widget(Clear, area);
    let chunks = Layout::default()
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .direction(Direction::Vertical)
        .margin(1)
        .split(area);

    let mut block = popup_block(format!(" {} ", browser.title()));
    let bottom = if browser.filtering || !browser.filter.is_empty() {
        format!(" filter: {}▏ ", browser.filter)
    } else {
        match browser.purpose {
            BrowserPurpose::Attach => " Enter: attach file · /: filter · Esc: cancel ".to_string(),
            BrowserPurpose::SaveTo(_) => {
                " s or [ Save here ]: save · /: filter · Esc: cancel ".to_string()
            }
        }
    };
    block = block.title_bottom(Line::styled(bottom, hint_style()).centered());
    f.render_widget(block, area);

    let header = match &browser.error {
        Some(error) => Line::styled(
            format!("{} ({error})", display_path(&browser.dir)),
            Style::default().fg(Color::Red),
        ),
        None => Line::styled(
            display_path(&browser.dir),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    };
    f.render_widget(Paragraph::new(header), chunks[0]);

    let name_width = (chunks[1].width as usize).saturating_sub(12);
    let items: Vec<ListItem> = browser
        .visible()
        .into_iter()
        .map(|entry| {
            if entry.is_save_here {
                ListItem::new(Line::styled(
                    entry.name.clone(),
                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                ))
            } else if entry.is_dir {
                ListItem::new(Line::styled(
                    format!("{}/", entry.name),
                    Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD),
                ))
            } else {
                let size = entry.size.map(human_size).unwrap_or_default();
                let name: String = entry.name.chars().take(name_width).collect();
                let pad = name_width.saturating_sub(name.width());
                ListItem::new(Line::from(vec![
                    Span::raw(name),
                    Span::raw(" ".repeat(pad)),
                    Span::styled(format!("{size:>10}"), hint_style()),
                ]))
            }
        })
        .collect();
    let len = items.len();
    if browser.state.selected().is_some_and(|i| i >= len) {
        browser.state.select(len.checked_sub(1));
    }
    let list = List::new(items).highlight_style(Style::default().reversed());
    f.render_stateful_widget(list, chunks[1], &mut browser.state);
}

fn draw_search(f: &mut Frame, app: &mut App) {
    let area = centered_rect(70, 60, f.area());
    f.render_widget(Clear, area);
    let chunks = Layout::default()
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .direction(Direction::Vertical)
        .split(area);

    let Some(search) = app.search.as_ref() else {
        return;
    };
    let query = search.input.data.clone();
    let cursor_col = search.input.cursor.col as u16;
    let width = chunks[1].width.saturating_sub(2) as usize;
    let items: Vec<ListItem> = search
        .hits
        .iter()
        .map(|hit| {
            let time = utc_timestamp_msec_to_local(hit.arrived_at)
                .format("%Y-%m-%d %H:%M ")
                .to_string();
            let name = app.name_by_id_cached(hit.from_id);
            let text = if hit.text.is_empty() {
                "[attachment]"
            } else {
                &hit.text
            };
            let budget = width.saturating_sub(time.width() + name.width() + 2);
            let text: String = text.chars().take(budget).collect();
            ListItem::new(Line::from(vec![
                Span::styled(time, Style::default().fg(Color::Yellow)),
                Span::styled(name, Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(": "),
                Span::raw(text),
            ]))
        })
        .collect();
    let count = items.len();
    let mut state = search.state.clone();

    let input = Paragraph::new(query.clone()).block(popup_block(" Find in chat "));
    f.render_widget(input, chunks[0]);
    f.set_cursor_position((chunks[0].x + cursor_col + 1, chunks[0].y + 1));

    let results_title = if query.trim().is_empty() {
        " Type to search ".to_string()
    } else {
        format!(" {count} result(s) · Enter: go to message · Esc: close ")
    };
    let list = List::new(items)
        .block(popup_block(results_title))
        .highlight_style(Style::default().reversed());
    f.render_stateful_widget(list, chunks[1], &mut state);
    if let Some(search) = app.search.as_mut() {
        search.state = state;
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Percentage((100 - percent_y) / 2),
                Constraint::Percentage(percent_y),
                Constraint::Percentage((100 - percent_y) / 2),
            ]
            .as_ref(),
        )
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            [
                Constraint::Percentage((100 - percent_x) / 2),
                Constraint::Percentage(percent_x),
                Constraint::Percentage((100 - percent_x) / 2),
            ]
            .as_ref(),
        )
        .split(popup_layout[1])[1]
}

#[cfg(test)]
mod tests {
    use crate::data::{AssociatedValue, BodyRange};
    use crate::signal::Attachment;

    use super::*;

    const USER_ID: Uuid = Uuid::nil();

    // formatting test options
    const PREFIX: &str = "                  ";
    const WIDTH: usize = 60;
    const HEIGHT: usize = 10;

    fn test_attachment() -> Attachment {
        Attachment {
            id: "2022-01-16T11:59:58.405665+00:00".to_string(),
            content_type: "image/jpeg".into(),
            filename: "/tmp/gurk/signal-2022-01-16T11:59:58.405665+00:00.jpg".into(),
            size: 238987,
        }
    }

    fn test_voice_note() -> Attachment {
        Attachment {
            id: "2022-01-16T12:00:00.000000+00:00".to_string(),
            content_type: "audio/aac".into(),
            filename: "/tmp/gurk/signal-2022-01-16T12:00:00.000000+00:00.aac".into(),
            size: 12345,
        }
    }

    fn name_resolver() -> NameResolver<'static> {
        NameResolver::single_user(USER_ID, "boxdot".to_string(), Color::Green)
    }

    fn test_message() -> Message {
        Message {
            from_id: USER_ID,
            message: None,
            arrived_at: 1642334397421,
            quote: None,
            attachments: Default::default(),
            reactions: Default::default(),
            receipt: Receipt::Sent,
            body_ranges: Default::default(),
            send_failed: Default::default(),
            attachment_downloads: Default::default(),
            edit: Default::default(),
            edited: Default::default(),
            deleted: Default::default(),
            expire_timer: None,
            expires_at: None,
        }
    }

    #[test]
    fn test_display_attachment_only_message() {
        let names = name_resolver();
        let msg = Message {
            attachments: vec![test_attachment()],
            ..test_message()
        };
        let rendered = display_message(
            &names,
            &msg,
            PREFIX,
            WIDTH,
            HEIGHT,
            ShowReceipt::Never,
            None,
            None,
            false,
        );

        let expected = ListItem::new(Text::from(vec![
            Line::from(vec![
                Span::styled("", Style::default().fg(Color::Yellow)),
                Span::styled(
                    display_time(msg.arrived_at),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled("boxdot", Style::default().fg(Color::Green)),
                Span::raw(": "),
                Span::raw("[image:"),
            ]),
            Line::from(vec![Span::raw(
                "                  signal-2022-01-16T11:59:58.405665+00:00.jpg]",
            )]),
        ]));
        assert_eq!(rendered, Some(expected));
    }

    #[test]
    fn test_display_text_and_attachment_message() {
        let names = name_resolver();
        let msg = Message {
            message: Some("Hello, World!".into()),
            attachments: vec![test_attachment()],
            ..test_message()
        };
        let rendered = display_message(
            &names,
            &msg,
            PREFIX,
            WIDTH,
            HEIGHT,
            ShowReceipt::Never,
            None,
            None,
            false,
        );

        let expected = ListItem::new(Text::from(vec![
            Line::from(vec![
                Span::styled("", Style::default().fg(Color::Yellow)),
                Span::styled(
                    display_time(msg.arrived_at),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled("boxdot", Style::default().fg(Color::Green)),
                Span::raw(": "),
                Span::raw("Hello, World!"),
            ]),
            Line::from(vec![Span::raw("                  [image:")]),
            Line::from(vec![Span::raw(
                "                  signal-2022-01-16T11:59:58.405665+00:00.jpg]",
            )]),
        ]));
        assert_eq!(rendered, Some(expected));
    }

    #[test]
    fn test_display_voice_note_message() {
        let names = name_resolver();
        let msg = Message {
            attachments: vec![test_voice_note()],
            ..test_message()
        };
        let rendered = display_message(
            &names,
            &msg,
            PREFIX,
            WIDTH,
            HEIGHT,
            ShowReceipt::Never,
            None,
            None,
            false,
        );

        let expected = ListItem::new(Text::from(vec![
            Line::from(vec![
                Span::styled("", Style::default().fg(Color::Yellow)),
                Span::styled(
                    display_time(msg.arrived_at),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled("boxdot", Style::default().fg(Color::Green)),
                Span::raw(": "),
                Span::raw("[voice note:"),
            ]),
            Line::from(vec![Span::raw(
                "                  signal-2022-01-16T12:00:00.000000+00:00.aac]",
            )]),
        ]));
        assert_eq!(rendered, Some(expected));
    }

    #[test]
    fn test_display_sent_receipt() {
        let names = name_resolver();
        let msg = Message {
            message: Some("Hello, World!".into()),
            receipt: Receipt::Sent,
            ..test_message()
        };
        let show_receipt = ShowReceipt::from_msg(&msg, USER_ID, true);
        let rendered = display_message(
            &names,
            &msg,
            PREFIX,
            WIDTH,
            HEIGHT,
            show_receipt,
            None,
            None,
            false,
        );

        let expected = ListItem::new(Text::from(vec![Line::from(vec![
            Span::styled("○ ", Style::default().fg(Color::Yellow)),
            Span::styled(
                display_time(msg.arrived_at),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled("boxdot", Style::default().fg(Color::Green)),
            Span::raw(": "),
            Span::raw("Hello, World!"),
        ])]));
        assert_eq!(rendered, Some(expected));
    }

    #[test]
    fn test_display_received_receipt() {
        let names = name_resolver();
        let msg = Message {
            message: Some("Hello, World!".into()),
            receipt: Receipt::Delivered,
            ..test_message()
        };
        let show_receipt = ShowReceipt::from_msg(&msg, USER_ID, true);
        let rendered = display_message(
            &names,
            &msg,
            PREFIX,
            WIDTH,
            HEIGHT,
            show_receipt,
            None,
            None,
            false,
        );

        let expected = ListItem::new(Text::from(vec![Line::from(vec![
            Span::styled("◉ ", Style::default().fg(Color::Yellow)),
            Span::styled(
                display_time(msg.arrived_at),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled("boxdot", Style::default().fg(Color::Green)),
            Span::raw(": "),
            Span::raw("Hello, World!"),
        ])]));
        assert_eq!(rendered, Some(expected));
    }

    #[test]
    fn test_display_delivered_receipt() {
        let names = name_resolver();
        let msg = Message {
            message: Some("Hello, World!".into()),
            receipt: Receipt::Read,
            ..test_message()
        };
        let show_receipt = ShowReceipt::from_msg(&msg, USER_ID, true);
        let rendered = display_message(
            &names,
            &msg,
            PREFIX,
            WIDTH,
            HEIGHT,
            show_receipt,
            None,
            None,
            false,
        );

        let expected = ListItem::new(Text::from(vec![Line::from(vec![
            Span::styled("● ", Style::default().fg(Color::Yellow)),
            Span::styled(
                display_time(msg.arrived_at),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled("boxdot", Style::default().fg(Color::Green)),
            Span::raw(": "),
            Span::raw("Hello, World!"),
        ])]));
        assert_eq!(rendered, Some(expected));
    }

    #[test]
    fn test_display_show_receipts_disabled() {
        let names = name_resolver();
        let msg = Message {
            message: Some("Hello, World!".into()),
            receipt: Receipt::Read,
            ..test_message()
        };
        let show_receipt = ShowReceipt::from_msg(&msg, USER_ID, false);
        let rendered = display_message(
            &names,
            &msg,
            PREFIX,
            WIDTH,
            HEIGHT,
            show_receipt,
            None,
            None,
            false,
        );

        let expected = ListItem::new(Text::from(vec![Line::from(vec![
            Span::styled("", Style::default().fg(Color::Yellow)),
            Span::styled(
                display_time(msg.arrived_at),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled("boxdot", Style::default().fg(Color::Green)),
            Span::raw(": "),
            Span::raw("Hello, World!"),
        ])]));
        assert_eq!(rendered, Some(expected));
    }

    #[test]
    fn test_display_receipts_for_incoming_message() {
        let user_id = Uuid::from_u128(1);
        let names = NameResolver::single_user(user_id, "boxdot".to_string(), Color::Green);
        let msg = Message {
            from_id: user_id,
            message: Some("Hello, World!".into()),
            receipt: Receipt::Read,
            ..test_message()
        };
        let show_receipt = ShowReceipt::from_msg(&msg, USER_ID, true);
        let rendered = display_message(
            &names,
            &msg,
            PREFIX,
            WIDTH,
            HEIGHT,
            show_receipt,
            None,
            None,
            false,
        );

        let expected = ListItem::new(Text::from(vec![Line::from(vec![
            Span::styled("  ", Style::default().fg(Color::Yellow)),
            Span::styled(
                display_time(msg.arrived_at),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled("boxdot", Style::default().fg(Color::Green)),
            Span::raw(": "),
            Span::raw("Hello, World!"),
        ])]));
        assert_eq!(rendered, Some(expected));
    }

    #[test]
    fn test_display_mention() {
        let user_id = Uuid::from_u128(1);
        let names = NameResolver::single_user(user_id, "boxdot".to_string(), Color::Green);
        let msg = Message {
            from_id: user_id,
            message: Some("Mention ￼  and even more ￼ . End".into()),
            receipt: Receipt::Read,
            body_ranges: vec![
                BodyRange {
                    start: 8,
                    end: 9,
                    value: AssociatedValue::MentionUuid(user_id),
                },
                BodyRange {
                    start: 25,
                    end: 26,
                    value: AssociatedValue::MentionUuid(user_id),
                },
            ],
            ..test_message()
        };
        let show_receipt = ShowReceipt::from_msg(&msg, USER_ID, true);
        let rendered = display_message(
            &names,
            &msg,
            PREFIX,
            WIDTH,
            HEIGHT,
            show_receipt,
            None,
            None,
            false,
        );

        let expected = ListItem::new(Text::from(vec![
            Line::from(vec![
                Span::styled("  ", Style::default().fg(Color::Yellow)),
                Span::styled(
                    display_time(msg.arrived_at),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled("boxdot", Style::default().fg(Color::Green)),
                Span::raw(": "),
                Span::raw("Mention @boxdot  and even more"),
            ]),
            Line::from(vec![Span::raw("                  @boxdot . End")]),
        ]));
        assert_eq!(rendered, Some(expected));
    }

    #[test]
    fn test_display_long_message_wraps() {
        let names = name_resolver();
        let msg = Message {
            message: Some(
                "This is a very long message that should wrap across multiple lines in the display"
                    .into(),
            ),
            ..test_message()
        };
        let rendered = display_message(
            &names,
            &msg,
            PREFIX,
            WIDTH,
            HEIGHT,
            ShowReceipt::Never,
            None,
            None,
            false,
        );

        let expected = ListItem::new(Text::from(vec![
            Line::from(vec![
                Span::styled("", Style::default().fg(Color::Yellow)),
                Span::styled(
                    display_time(msg.arrived_at),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled("boxdot", Style::default().fg(Color::Green)),
                Span::raw(": "),
                Span::raw("This is a very long message that"),
            ]),
            Line::from(vec![Span::raw(
                "                  should wrap across multiple lines in the",
            )]),
            Line::from(vec![Span::raw("                  display")]),
        ]));
        assert_eq!(rendered, Some(expected));
    }

    #[test]
    fn test_display_unread_messages_division() {
        let names = name_resolver();
        let msg = Message {
            message: Some("Hello, World!".into()),
            ..test_message()
        };
        let division = "--new messages--".to_owned();
        let rendered = display_message(
            &names,
            &msg,
            PREFIX,
            WIDTH,
            HEIGHT,
            ShowReceipt::Never,
            None,
            Some(division.clone()),
            false,
        );

        let expected = ListItem::new(Text::from(vec![
            Line::from(division),
            Line::from(vec![
                Span::styled("", Style::default().fg(Color::Yellow)),
                Span::styled(
                    display_time(msg.arrived_at),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled("boxdot", Style::default().fg(Color::Green)),
                Span::raw(": "),
                Span::raw("Hello, World!"),
            ]),
        ]));
        assert_eq!(rendered, Some(expected));
    }
}
