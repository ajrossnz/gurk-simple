//! Minimal TUI file browser used for attaching files and choosing a download directory

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::storage::MessageId;

/// What the file browser was opened for
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserPurpose {
    /// Pick a file to attach to the message being composed
    Attach,
    /// Pick a directory to save the attachments of a message into
    SaveTo(MessageId),
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: Option<u64>,
    /// Pseudo entry: "save into the current directory"
    pub is_save_here: bool,
}

/// Result of feeding a key into the browser
#[derive(Debug, PartialEq, Eq)]
pub enum BrowserAction {
    None,
    Cancel,
    /// A file was chosen (attach) or a directory was chosen (save to)
    Chosen(PathBuf),
}

pub struct FileBrowser {
    pub purpose: BrowserPurpose,
    pub dir: PathBuf,
    pub entries: Vec<Entry>,
    pub state: ListState,
    pub show_hidden: bool,
    /// Type-ahead filter; active while `filtering` is true
    pub filter: String,
    pub filtering: bool,
    pub error: Option<String>,
}

impl FileBrowser {
    pub fn open(purpose: BrowserPurpose, dir: PathBuf) -> Self {
        let dir = if dir.is_dir() { dir } else { home_dir() };
        let mut browser = Self {
            purpose,
            dir,
            entries: Vec::new(),
            state: ListState::default(),
            show_hidden: false,
            filter: String::new(),
            filtering: false,
            error: None,
        };
        browser.reload(None);
        browser
    }

    pub fn title(&self) -> &'static str {
        match self.purpose {
            BrowserPurpose::Attach => "Attach a file",
            BrowserPurpose::SaveTo(_) => "Save attachments to directory",
        }
    }

    /// Entries matching the current filter
    pub fn visible(&self) -> Vec<&Entry> {
        let filter = self.filter.to_lowercase();
        self.entries
            .iter()
            .filter(|e| {
                e.is_save_here || filter.is_empty() || e.name.to_lowercase().contains(&filter)
            })
            .collect()
    }

    fn reload(&mut self, select_name: Option<&str>) {
        self.entries.clear();
        self.error = None;
        if matches!(self.purpose, BrowserPurpose::SaveTo(_)) {
            self.entries.push(Entry {
                name: "[ Save here ]".to_string(),
                path: self.dir.clone(),
                is_dir: true,
                size: None,
                is_save_here: true,
            });
        }
        if let Some(parent) = self.dir.parent() {
            self.entries.push(Entry {
                name: "..".to_string(),
                path: parent.to_path_buf(),
                is_dir: true,
                size: None,
                is_save_here: false,
            });
        }
        match std::fs::read_dir(&self.dir) {
            Ok(read_dir) => {
                let mut entries: Vec<Entry> = read_dir
                    .filter_map(Result::ok)
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().into_owned();
                        if !self.show_hidden && name.starts_with('.') {
                            return None;
                        }
                        let path = e.path();
                        // follow symlinks
                        let meta = std::fs::metadata(&path).ok();
                        let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
                        let is_attach = matches!(self.purpose, BrowserPurpose::Attach);
                        if !is_dir && !is_attach {
                            // only directories are relevant when choosing where to save
                            return None;
                        }
                        Some(Entry {
                            name,
                            path,
                            is_dir,
                            size: meta.filter(|m| m.is_file()).map(|m| m.len()),
                            is_save_here: false,
                        })
                    })
                    .collect();
                entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
                    (true, false) => Ordering::Less,
                    (false, true) => Ordering::Greater,
                    _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                });
                self.entries.extend(entries);
            }
            Err(error) => {
                warn!(dir = %self.dir.display(), %error, "failed to read directory");
                self.error = Some(error.to_string());
            }
        }
        let idx = select_name
            .and_then(|name| self.visible().iter().position(|e| e.name == name))
            .unwrap_or(0);
        self.state.select(Some(idx));
    }

    fn change_dir(&mut self, dir: PathBuf) {
        let previous = self
            .dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        let going_up = self.dir.parent() == Some(dir.as_path());
        self.dir = dir;
        self.filter.clear();
        self.filtering = false;
        self.reload(if going_up { previous.as_deref() } else { None });
    }

    fn go_up(&mut self) {
        if let Some(parent) = self.dir.parent() {
            self.change_dir(parent.to_path_buf());
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let len = self.visible().len();
        if len == 0 {
            self.state.select(None);
            return;
        }
        let cur = self.state.selected().unwrap_or(0) as isize;
        let next = (cur + delta).clamp(0, len as isize - 1);
        self.state.select(Some(next as usize));
    }

    fn selected_entry(&self) -> Option<Entry> {
        let idx = self.state.selected()?;
        self.visible().get(idx).map(|e| (*e).clone())
    }

    fn activate(&mut self) -> BrowserAction {
        let Some(entry) = self.selected_entry() else {
            return BrowserAction::None;
        };
        if entry.is_save_here {
            BrowserAction::Chosen(self.dir.clone())
        } else if entry.is_dir {
            self.change_dir(entry.path);
            BrowserAction::None
        } else {
            BrowserAction::Chosen(entry.path)
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> BrowserAction {
        if self.filtering {
            match key.code {
                KeyCode::Esc => {
                    self.filter.clear();
                    self.filtering = false;
                    self.state.select(Some(0));
                }
                KeyCode::Enter => {
                    self.filtering = false;
                    return self.activate();
                }
                KeyCode::Backspace => {
                    if self.filter.pop().is_none() {
                        self.filtering = false;
                    }
                    self.state.select(Some(0));
                }
                KeyCode::Up => self.move_selection(-1),
                KeyCode::Down => self.move_selection(1),
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.filter.push(c);
                    self.state.select(Some(0));
                }
                _ => {}
            }
            return BrowserAction::None;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return BrowserAction::Cancel,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return BrowserAction::Cancel;
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-10),
            KeyCode::PageDown => self.move_selection(10),
            KeyCode::Home | KeyCode::Char('g') => self.state.select(Some(0)),
            KeyCode::End | KeyCode::Char('G') => self.move_selection(isize::MAX / 2),
            KeyCode::Left | KeyCode::Backspace | KeyCode::Char('h') => self.go_up(),
            KeyCode::Right | KeyCode::Char('l') => {
                if self
                    .selected_entry()
                    .is_some_and(|e| e.is_dir && !e.is_save_here)
                {
                    return self.activate();
                }
            }
            KeyCode::Enter => return self.activate(),
            KeyCode::Char('s') if matches!(self.purpose, BrowserPurpose::SaveTo(_)) => {
                return BrowserAction::Chosen(self.dir.clone());
            }
            KeyCode::Char('.') => {
                self.show_hidden = !self.show_hidden;
                self.reload(None);
            }
            KeyCode::Char('~') => self.change_dir(home_dir()),
            KeyCode::Char('/') => {
                self.filtering = true;
                self.filter.clear();
            }
            _ => {}
        }
        BrowserAction::None
    }
}

pub fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

pub fn downloads_dir() -> PathBuf {
    dirs::download_dir().unwrap_or_else(|| home_dir().join("Downloads"))
}

/// Shortens a path by replacing the home directory with `~`
pub fn display_path(path: &Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        if rest.as_os_str().is_empty() {
            return "~".to_string();
        }
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

pub fn human_size(size: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = size as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{size} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Copies `src` into `dir`, avoiding overwriting existing files. Returns the destination.
pub fn copy_into_dir(src: &Path, dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "attachment".to_string());
    let mut dest = dir.join(&name);
    let mut idx = 0;
    while dest.exists() {
        idx += 1;
        let p = Path::new(&name);
        dest = match (p.file_stem(), p.extension()) {
            (Some(stem), Some(ext)) => dir.join(format!(
                "{}.{idx}.{}",
                stem.to_string_lossy(),
                ext.to_string_lossy()
            )),
            _ => dir.join(format!("{name}.{idx}")),
        };
    }
    std::fs::copy(src, &dest)?;
    Ok(dest)
}

/// Small piece of UI state persisted between runs
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UiState {
    pub last_attach_dir: Option<PathBuf>,
    pub last_save_dir: Option<PathBuf>,
}

impl UiState {
    fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("ui_state.toml")
    }

    pub fn load(data_dir: &Path) -> Self {
        std::fs::read_to_string(Self::path(data_dir))
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, data_dir: &Path) {
        let result = toml::to_string(self)
            .map_err(|e| e.to_string())
            .and_then(|s| std::fs::write(Self::path(data_dir), s).map_err(|e| e.to_string()));
        if let Err(error) = result {
            warn!(%error, "failed to save ui state");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn browse_and_choose_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        std::fs::write(tmp.path().join("sub/b.txt"), b"b").unwrap();
        std::fs::write(tmp.path().join("a.txt"), b"a").unwrap();
        std::fs::write(tmp.path().join(".hidden"), b"h").unwrap();

        let mut fb = FileBrowser::open(BrowserPurpose::Attach, tmp.path().to_path_buf());
        let names: Vec<_> = fb.visible().iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, ["..", "sub", "a.txt"]);

        // enter "sub", then choose b.txt
        fb.on_key(key(KeyCode::Down));
        assert_eq!(fb.on_key(key(KeyCode::Enter)), BrowserAction::None);
        assert_eq!(fb.dir, tmp.path().join("sub"));
        fb.on_key(key(KeyCode::Down));
        assert_eq!(
            fb.on_key(key(KeyCode::Enter)),
            BrowserAction::Chosen(tmp.path().join("sub/b.txt"))
        );

        // going up selects the directory we came from
        fb.on_key(key(KeyCode::Char('h')));
        assert_eq!(fb.dir, tmp.path());
        assert_eq!(fb.selected_entry().unwrap().name, "sub");

        // filter
        fb.on_key(key(KeyCode::Char('/')));
        fb.on_key(key(KeyCode::Char('a')));
        let names: Vec<_> = fb.visible().iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, ["a.txt"]);
        assert_eq!(
            fb.on_key(key(KeyCode::Enter)),
            BrowserAction::Chosen(tmp.path().join("a.txt"))
        );
    }

    #[test]
    fn save_to_lists_only_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        std::fs::write(tmp.path().join("a.txt"), b"a").unwrap();
        let id = MessageId::new(crate::data::ChannelId::User(uuid::Uuid::nil()), 1);
        let mut fb = FileBrowser::open(BrowserPurpose::SaveTo(id), tmp.path().to_path_buf());
        let names: Vec<_> = fb.visible().iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, ["[ Save here ]", "..", "sub"]);
        assert_eq!(
            fb.on_key(key(KeyCode::Enter)),
            BrowserAction::Chosen(tmp.path().to_path_buf())
        );
    }

    #[test]
    fn copy_into_dir_avoids_conflicts() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("photo.jpg");
        std::fs::write(&src, b"x").unwrap();
        let dest_dir = tmp.path().join("out");
        let first = copy_into_dir(&src, &dest_dir).unwrap();
        let second = copy_into_dir(&src, &dest_dir).unwrap();
        assert_eq!(first, dest_dir.join("photo.jpg"));
        assert_eq!(second, dest_dir.join("photo.1.jpg"));
    }
}
