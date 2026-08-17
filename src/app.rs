use std::sync::Arc;
use std::{cell::Cell, sync::mpsc};

use crate::event::{AppEvent, Event, EventHandler};
use crate::{
    chat::{ChatRef, ChatSummary, Message, Profile, SimplexEvent, User},
    preferences::Preferences,
    simplex::SimplexApi,
    simplex_worker::SimplexCommand,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::{DefaultTerminal, layout::Rect};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Section {
    #[default]
    Chats,
    Profiles,
    Settings,
}

#[derive(Clone, Debug)]
pub enum StartupState {
    Loading,
    NoActiveUser,
    Ready(User),
    Failed(String),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum InputMode {
    #[default]
    None,
    CreateProfile,
}

/// All state owned by the user interface.
#[derive(Debug)]
pub struct App {
    pub running: bool,
    pub section: Section,
    pub selected_chat: usize,
    pub selected_profile: usize,
    pub selected_setting: usize,
    pub startup: StartupState,
    pub chats: Vec<ChatSummary>,
    pub profiles: Vec<Profile>,
    pub messages: Vec<Message>,
    pub loaded_chat: Option<ChatRef>,
    pub chat_loading: bool,
    pub chat_error: Option<String>,
    pub composer: String,
    pub composer_focused: bool,
    pub sending: bool,
    pub preferences: Preferences,
    pub auto_delete_seconds: i64,
    pub input_mode: InputMode,
    pub input: String,
    pub notice: Option<String>,
    /// Last terminal area, used to translate mouse coordinates into UI actions.
    pub(crate) area: Cell<Rect>,
    pub(crate) composer_area: Cell<Rect>,
    pub(crate) send_area: Cell<Rect>,
    pub events: EventHandler,
    simplex_events: mpsc::Receiver<SimplexEvent>,
    simplex_commands: mpsc::Sender<SimplexCommand>,
}

impl Default for App {
    fn default() -> Self {
        let (_simplex_sender, simplex_events) = mpsc::channel();
        let (simplex_commands, _simplex_commands) = mpsc::channel();
        Self {
            running: true,
            section: Section::Chats,
            selected_chat: 0,
            selected_profile: 0,
            selected_setting: 0,
            startup: StartupState::Loading,
            chats: Vec::new(),
            profiles: Vec::new(),
            messages: Vec::new(),
            loaded_chat: None,
            chat_loading: false,
            chat_error: None,
            composer: String::new(),
            composer_focused: false,
            sending: false,
            preferences: Preferences::default(),
            auto_delete_seconds: 0,
            input_mode: InputMode::None,
            input: String::new(),
            notice: None,
            area: Cell::new(Rect::default()),
            composer_area: Cell::new(Rect::default()),
            send_area: Cell::new(Rect::default()),
            events: EventHandler::new(),
            simplex_events,
            simplex_commands,
        }
    }
}

impl App {
    pub const SETTINGS: [&'static str; 5] = [
        "General",
        "Notifications",
        "Privacy & Security",
        "Appearance",
        "About",
    ];

    pub fn new(api: Arc<SimplexApi>) -> Self {
        let mut app = Self::default();
        if let Ok(paths) = crate::simplex::SimplexPaths::discover() {
            app.preferences = Preferences::load(&paths.root);
        }
        let (sender, receiver) = mpsc::channel();
        app.simplex_events = receiver;
        app.simplex_commands = crate::simplex_worker::spawn(api, sender);
        app
    }

    pub async fn run(mut self, mut terminal: DefaultTerminal) -> color_eyre::Result<()> {
        while self.running {
            terminal.draw(|frame| frame.render_widget(&self, frame.area()))?;
            match self.events.next().await? {
                Event::Tick => self.tick(),
                Event::Crossterm(event) => match event {
                    crossterm::event::Event::Key(key)
                        if key.kind == crossterm::event::KeyEventKind::Press =>
                    {
                        self.handle_key_events(key)?
                    }
                    crossterm::event::Event::Mouse(mouse) => {
                        self.handle_mouse_event(mouse.kind, mouse.column, mouse.row)
                    }
                    _ => {}
                },
                Event::App(app_event) => self.handle_app_event(app_event),
            }
        }
        Ok(())
    }

    pub fn handle_key_events(&mut self, key: KeyEvent) -> color_eyre::Result<()> {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.events.send(AppEvent::Quit);
            return Ok(());
        }
        if self.input_mode == InputMode::CreateProfile {
            match key.code {
                KeyCode::Esc => {
                    self.input_mode = InputMode::None;
                    self.input.clear();
                }
                KeyCode::Enter if !self.input.trim().is_empty() => {
                    let name = self.input.trim().to_owned();
                    self.input_mode = InputMode::None;
                    self.input.clear();
                    self.notice = Some("Creating profile…".into());
                    self.startup = StartupState::Loading;
                    self.chats.clear();
                    self.messages.clear();
                    self.loaded_chat = None;
                    let _ = self
                        .simplex_commands
                        .send(SimplexCommand::CreateProfile(name));
                }
                KeyCode::Backspace => {
                    self.input.pop();
                }
                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.input.push(character)
                }
                _ => {}
            }
            return Ok(());
        }
        if self.composer_focused {
            match key.code {
                KeyCode::Esc => self.composer_focused = false,
                KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    self.composer.push('\n')
                }
                KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => {
                    self.composer.push('\n')
                }
                KeyCode::Enter => self.send_message(),
                KeyCode::Backspace => {
                    self.composer.pop();
                }
                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.composer.push(character)
                }
                _ => {}
            }
            return Ok(());
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.events.send(AppEvent::Quit),
            KeyCode::Enter | KeyCode::Char('i')
                if self.section == Section::Chats && !self.chats.is_empty() =>
            {
                self.composer_focused = true
            }
            KeyCode::Enter if self.section == Section::Profiles => self.activate_profile(),
            KeyCode::Char('n') if self.section == Section::Profiles => {
                self.input_mode = InputMode::CreateProfile;
                self.input.clear();
            }
            KeyCode::Enter | KeyCode::Char(' ') if self.section == Section::Settings => {
                self.activate_setting()
            }
            KeyCode::Char('p')
                if self.section == Section::Settings && self.selected_setting == 1 =>
            {
                self.preferences.message_preview = !self.preferences.message_preview;
                self.save_preferences();
            }
            KeyCode::Char('c')
                if self.section == Section::Settings && self.selected_setting == 3 =>
            {
                self.preferences.compact_messages = !self.preferences.compact_messages;
                self.save_preferences();
            }
            KeyCode::Tab => self.events.send(AppEvent::ToggleSection),
            KeyCode::Char('1') => self.events.send(AppEvent::SelectSection(Section::Chats)),
            KeyCode::Char('2') => self.events.send(AppEvent::SelectSection(Section::Profiles)),
            KeyCode::Char('3') => self.events.send(AppEvent::SelectSection(Section::Settings)),
            KeyCode::Up | KeyCode::Char('k') => self.events.send(AppEvent::SelectPrevious),
            KeyCode::Down | KeyCode::Char('j') => self.events.send(AppEvent::SelectNext),
            KeyCode::Left | KeyCode::Char('h') => self.events.send(AppEvent::PreviousSection),
            KeyCode::Right | KeyCode::Char('l') => self.events.send(AppEvent::NextSection),
            _ => {}
        }
        Ok(())
    }

    fn handle_mouse_event(&mut self, kind: MouseEventKind, column: u16, row: u16) {
        if matches!(kind, MouseEventKind::Down(MouseButton::Left)) {
            if self.send_area.get().contains((column, row).into()) {
                self.send_message();
                return;
            }
            if self.composer_area.get().contains((column, row).into()) {
                self.composer_focused = true;
                return;
            }
        }
        let area = self.area.get();
        let sidebar_width = area.width * 32 / 100;
        if column >= area.x + sidebar_width || row < area.y || row >= area.bottom() {
            return;
        }

        match kind {
            MouseEventKind::Down(MouseButton::Left) if row == area.y + 1 => {
                // Tabs renders " Chats │ Profiles │ Settings " from the inner left edge.
                let relative_x = column.saturating_sub(area.x + 1);
                let section = if relative_x < 8 {
                    Section::Chats
                } else if relative_x < 19 {
                    Section::Profiles
                } else {
                    Section::Settings
                };
                self.handle_app_event(AppEvent::SelectSection(section));
            }
            MouseEventKind::Down(MouseButton::Left) if row >= area.y + 5 => {
                // Three tab rows, one list border and one padding row precede the items.
                let index = usize::from(row - area.y - 5);
                self.handle_app_event(AppEvent::SelectIndex(index));
            }
            MouseEventKind::ScrollUp => self.handle_app_event(AppEvent::SelectPrevious),
            MouseEventKind::ScrollDown => self.handle_app_event(AppEvent::SelectNext),
            _ => {}
        }
    }

    fn handle_app_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Quit => self.running = false,
            AppEvent::ToggleSection => {
                self.section = match self.section {
                    Section::Chats => Section::Profiles,
                    Section::Profiles => Section::Settings,
                    Section::Settings => Section::Chats,
                }
            }
            AppEvent::PreviousSection => {
                self.section = match self.section {
                    Section::Chats => Section::Settings,
                    Section::Profiles => Section::Chats,
                    Section::Settings => Section::Profiles,
                }
            }
            AppEvent::NextSection => {
                self.section = match self.section {
                    Section::Chats => Section::Profiles,
                    Section::Profiles => Section::Settings,
                    Section::Settings => Section::Chats,
                }
            }
            AppEvent::SelectSection(section) => self.section = section,
            AppEvent::SelectPrevious => {
                let selected = self.selected_index_mut();
                *selected = selected.saturating_sub(1);
            }
            AppEvent::SelectNext => {
                let max = self.item_count().saturating_sub(1);
                let selected = self.selected_index_mut();
                *selected = (*selected + 1).min(max);
            }
            AppEvent::SelectIndex(index) => {
                if index < self.item_count() {
                    *self.selected_index_mut() = index;
                }
            }
        }
    }

    fn selected_index_mut(&mut self) -> &mut usize {
        match self.section {
            Section::Chats => &mut self.selected_chat,
            Section::Profiles => &mut self.selected_profile,
            Section::Settings => &mut self.selected_setting,
        }
    }

    fn item_count(&self) -> usize {
        match self.section {
            Section::Chats => self.chats.len(),
            Section::Profiles => self.profiles.len().saturating_add(1),
            Section::Settings => Self::SETTINGS.len(),
        }
    }

    pub fn tick(&mut self) {
        while let Ok(event) = self.simplex_events.try_recv() {
            match event {
                SimplexEvent::Ready { user, chats } => {
                    self.startup = StartupState::Ready(user);
                    self.chats = chats;
                    self.selected_chat = self.selected_chat.min(self.chats.len().saturating_sub(1));
                }
                SimplexEvent::ProfilesLoaded(profiles) => {
                    self.profiles = profiles;
                    self.sync_selected_profile();
                }
                SimplexEvent::ProfileActivated { user, chats } => {
                    self.startup = StartupState::Ready(user);
                    self.chats = chats;
                    self.messages.clear();
                    self.loaded_chat = None;
                    self.notice = Some("Profile activated".into());
                    for profile in &mut self.profiles {
                        profile.active = matches!(&self.startup, StartupState::Ready(active) if active.id == profile.id);
                    }
                    self.sync_selected_profile();
                }
                SimplexEvent::ProfileCreated {
                    user,
                    profiles,
                    chats,
                } => {
                    self.startup = StartupState::Ready(user);
                    self.profiles = profiles;
                    self.chats = chats;
                    self.messages.clear();
                    self.loaded_chat = None;
                    self.notice = Some("Profile created".into());
                    self.sync_selected_profile();
                }
                SimplexEvent::SettingChanged(message) => self.notice = Some(message),
                SimplexEvent::AutoDeleteLoaded(seconds) => self.auto_delete_seconds = seconds,
                SimplexEvent::ChatLoaded { chat_ref, messages } => {
                    if self.selected_chat_ref() == Some(&chat_ref) {
                        self.loaded_chat = Some(chat_ref);
                        self.chat_loading = false;
                        self.messages = messages;
                        self.chat_error = None;
                    }
                }
                SimplexEvent::MessageReceived { chat_ref, message } => {
                    if self.loaded_chat.as_ref() == Some(&chat_ref) {
                        if !self.messages.iter().any(|item| item.id == message.id) {
                            self.messages.push(message);
                        }
                    } else if let Some(chat) =
                        self.chats.iter_mut().find(|chat| chat.chat_ref == chat_ref)
                    {
                        chat.unread_count += 1;
                    }
                }
                SimplexEvent::ChatLoadFailed { chat_ref, error } => {
                    if self.selected_chat_ref() == Some(&chat_ref) {
                        self.loaded_chat = Some(chat_ref);
                        self.chat_loading = false;
                        self.messages.clear();
                        self.chat_error = Some(error);
                    }
                }
                SimplexEvent::MessageSent { chat_ref, text } => {
                    if self.selected_chat_ref() == Some(&chat_ref) {
                        if self.composer == text {
                            self.composer.clear();
                        }
                        self.sending = false;
                        self.chat_error = None;
                    }
                }
                SimplexEvent::MessageSendFailed { chat_ref, error } => {
                    if self.selected_chat_ref() == Some(&chat_ref) {
                        self.sending = false;
                        self.chat_error = Some(error);
                    }
                }
                SimplexEvent::NoActiveUser => self.startup = StartupState::NoActiveUser,
                SimplexEvent::Failed(error) => self.startup = StartupState::Failed(error),
            }
        }
        if let Some(chat_ref) = self.selected_chat_ref().cloned()
            && self.loaded_chat.as_ref() != Some(&chat_ref)
        {
            self.loaded_chat = Some(chat_ref.clone());
            self.messages.clear();
            self.chat_loading = true;
            self.chat_error = None;
            let _ = self
                .simplex_commands
                .send(SimplexCommand::LoadChat(chat_ref));
        }
    }

    fn selected_chat_ref(&self) -> Option<&ChatRef> {
        self.chats
            .get(self.selected_chat)
            .map(|chat| &chat.chat_ref)
    }

    fn send_message(&mut self) {
        if self.composer.trim().is_empty() || self.sending {
            return;
        }
        let text = self.composer.clone();
        let Some(chat_ref) = self.selected_chat_ref().cloned() else {
            return;
        };
        self.sending = true;
        self.chat_error = None;
        if self
            .simplex_commands
            .send(SimplexCommand::SendMessage { chat_ref, text })
            .is_err()
        {
            self.sending = false;
            self.chat_error = Some("SimpleX worker is not available".into());
        }
    }

    fn activate_profile(&mut self) {
        let Some(profile) = self.profiles.get(self.selected_profile) else {
            self.input_mode = InputMode::CreateProfile;
            self.input.clear();
            return;
        };
        if profile.active {
            self.notice = Some("Profile is already active".into());
            return;
        }
        self.notice = Some("Switching profile…".into());
        self.startup = StartupState::Loading;
        self.chats.clear();
        self.messages.clear();
        self.loaded_chat = None;
        let _ = self
            .simplex_commands
            .send(SimplexCommand::ActivateProfile(profile.id));
    }

    fn activate_setting(&mut self) {
        match self.selected_setting {
            1 => {
                let Some(user) = self.active_user().cloned() else {
                    self.notice = Some("Create a profile first".into());
                    return;
                };
                let enabled = !user.notifications;
                if let StartupState::Ready(active) = &mut self.startup {
                    active.notifications = enabled;
                }
                if let Some(profile) = self.profiles.iter_mut().find(|p| p.id == user.id) {
                    profile.notifications = enabled;
                }
                let _ = self
                    .simplex_commands
                    .send(SimplexCommand::SetNotifications {
                        user_id: user.id,
                        enabled,
                    });
            }
            2 => {
                let Some(user_id) = self.active_user().map(|user| user.id) else {
                    self.notice = Some("Create a profile first".into());
                    return;
                };
                self.auto_delete_seconds = match self.auto_delete_seconds {
                    0 => 86_400,
                    86_400 => 604_800,
                    604_800 => 2_592_000,
                    _ => 0,
                };
                let _ = self.simplex_commands.send(SimplexCommand::SetAutoDelete {
                    user_id,
                    seconds: self.auto_delete_seconds,
                });
            }
            3 => {
                self.preferences.theme = self.preferences.theme.next();
                self.save_preferences();
            }
            _ => {}
        }
    }

    pub fn active_user(&self) -> Option<&User> {
        match &self.startup {
            StartupState::Ready(user) => Some(user),
            _ => None,
        }
    }

    fn sync_selected_profile(&mut self) {
        if let Some(index) = self.profiles.iter().position(|profile| profile.active) {
            self.selected_profile = index;
        } else {
            self.selected_profile = self
                .selected_profile
                .min(self.profiles.len().saturating_sub(1));
        }
    }

    fn save_preferences(&mut self) {
        match crate::simplex::SimplexPaths::discover().and_then(|paths| {
            paths
                .create()
                .map_err(|_| crate::simplex::SimplexError::HomeDirectory)?;
            self.preferences
                .save(&paths.root)
                .map_err(|_| crate::simplex::SimplexError::HomeDirectory)
        }) {
            Ok(()) => self.notice = Some("Setting saved".into()),
            Err(error) => self.notice = Some(format!("Could not save setting: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn navigation_keeps_separate_selections() {
        let mut app = App {
            chats: vec![
                ChatSummary {
                    chat_ref: ChatRef("@1".into()),
                    display_name: "alice".into(),
                    unread_count: 0,
                },
                ChatSummary {
                    chat_ref: ChatRef("@2".into()),
                    display_name: "bob".into(),
                    unread_count: 0,
                },
            ],
            profiles: vec![Profile {
                id: 1,
                display_name: "personal".into(),
                notifications: true,
                active: true,
            }],
            ..App::default()
        };
        app.handle_app_event(AppEvent::SelectNext);
        app.handle_app_event(AppEvent::SelectSection(Section::Profiles));
        app.handle_app_event(AppEvent::SelectNext);
        app.handle_app_event(AppEvent::SelectSection(Section::Settings));
        app.handle_app_event(AppEvent::SelectNext);
        app.handle_app_event(AppEvent::SelectNext);
        assert_eq!(app.selected_chat, 1);
        assert_eq!(app.selected_profile, 1);
        assert_eq!(app.selected_setting, 2);
    }

    #[tokio::test]
    async fn mouse_selects_tabs_and_the_exact_list_row() {
        let mut app = App::default();
        app.area.set(Rect::new(0, 0, 100, 30));

        app.handle_mouse_event(MouseEventKind::Down(MouseButton::Left), 12, 1);
        assert_eq!(app.section, Section::Profiles);

        app.handle_mouse_event(MouseEventKind::Down(MouseButton::Left), 23, 1);
        assert_eq!(app.section, Section::Settings);

        app.handle_mouse_event(MouseEventKind::Down(MouseButton::Left), 3, 7);
        assert_eq!(app.selected_setting, 2);

        app.handle_mouse_event(MouseEventKind::Down(MouseButton::Left), 3, 1);
        assert_eq!(app.section, Section::Chats);
    }

    #[tokio::test]
    async fn enter_sends_but_shift_enter_inserts_a_newline() {
        let (commands, receiver) = mpsc::channel();
        let mut app = App {
            chats: vec![ChatSummary {
                chat_ref: ChatRef("@7".into()),
                display_name: "alice".into(),
                unread_count: 0,
            }],
            composer_focused: true,
            simplex_commands: commands,
            ..App::default()
        };

        app.handle_key_events(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
            .unwrap();
        app.handle_key_events(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT))
            .unwrap();
        app.handle_key_events(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.composer, "a\nb");

        app.handle_key_events(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        let SimplexCommand::SendMessage { chat_ref, text } = receiver.try_recv().unwrap() else {
            panic!("expected send command")
        };
        assert_eq!(chat_ref, ChatRef("@7".into()));
        assert_eq!(text, "a\nb");
        assert!(app.sending);
    }

    #[tokio::test]
    async fn clicking_send_button_sends_the_draft() {
        let (commands, receiver) = mpsc::channel();
        let mut app = App {
            chats: vec![ChatSummary {
                chat_ref: ChatRef("#3".into()),
                display_name: "team".into(),
                unread_count: 0,
            }],
            composer: "hello team".into(),
            simplex_commands: commands,
            ..App::default()
        };
        app.send_area.set(Rect::new(70, 20, 10, 5));

        app.handle_mouse_event(MouseEventKind::Down(MouseButton::Left), 75, 22);

        let SimplexCommand::SendMessage { chat_ref, text } = receiver.try_recv().unwrap() else {
            panic!("expected send command")
        };
        assert_eq!(chat_ref, ChatRef("#3".into()));
        assert_eq!(text, "hello team");
    }
}
