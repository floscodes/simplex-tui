use std::sync::Arc;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    io::{self, Write},
    sync::mpsc,
};

use crate::event::{AppEvent, Event, EventHandler};
use crate::{
    chat::{
        ChatDeletionSettings, ChatFeatures, ChatRef, ChatSummary, Message, Profile, ServerEntry,
        ServerProtocol, SimplexEvent, User,
    },
    preferences::Preferences,
    simplex::SimplexApi,
    simplex_worker::{ChatFeature, SimplexCommand},
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
    ConfirmDeleteProfile,
    AddServer,
}

#[derive(Clone, Debug)]
pub(crate) struct MessageHitbox {
    pub area: Rect,
    pub item_id: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct ReactionPicker {
    pub chat_ref: ChatRef,
    pub item_id: i64,
    pub column: u16,
    pub row: u16,
}

#[derive(Clone, Debug)]
pub(crate) struct ChatDeletionDialog {
    pub chat_ref: ChatRef,
    pub settings: Option<ChatDeletionSettings>,
    pub pending: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct DownloadCancelDialog {
    pub file_id: i64,
    pub file_name: String,
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
    pub chat_scroll: usize,
    pub new_messages_below: usize,
    pub composer: String,
    pub composer_focused: bool,
    pub sending: bool,
    pub preferences: Preferences,
    pub auto_delete_seconds: i64,
    pub auto_delete_pending: Option<i64>,
    pub servers: Vec<ServerEntry>,
    pub server_protocol: ServerProtocol,
    pub selected_server: usize,
    pub chat_features: ChatFeatures,
    pub invitation_link: Option<String>,
    pub invitation_loading: bool,
    pub invitation_error: Option<String>,
    pub input_mode: InputMode,
    pub input: String,
    pub notice: Option<String>,
    /// Last terminal area, used to translate mouse coordinates into UI actions.
    pub(crate) area: Cell<Rect>,
    pub(crate) composer_area: Cell<Rect>,
    pub(crate) send_area: Cell<Rect>,
    pub(crate) jump_to_latest_area: Cell<Rect>,
    pub(crate) max_chat_scroll: Cell<usize>,
    pub(crate) delete_cancel_area: Cell<Rect>,
    pub(crate) delete_ok_area: Cell<Rect>,
    pub(crate) chat_deletion_close_area: Cell<Rect>,
    pub(crate) chat_deletion_change_area: Cell<Rect>,
    pub(crate) message_hitboxes: RefCell<Vec<MessageHitbox>>,
    pub(crate) reaction_picker: Option<ReactionPicker>,
    pub(crate) reaction_option_areas: RefCell<Vec<(Rect, String)>>,
    pub(crate) chat_deletion_dialog: Option<ChatDeletionDialog>,
    pub(crate) download_cancel_dialog: Option<DownloadCancelDialog>,
    pub(crate) download_cancel_no_area: Cell<Rect>,
    pub(crate) download_cancel_yes_area: Cell<Rect>,
    pub events: EventHandler,
    message_cache: HashMap<ChatRef, Vec<Message>>,
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
            chat_scroll: 0,
            new_messages_below: 0,
            composer: String::new(),
            composer_focused: false,
            sending: false,
            preferences: Preferences::default(),
            auto_delete_seconds: 0,
            auto_delete_pending: None,
            servers: Vec::new(),
            server_protocol: ServerProtocol::Smp,
            selected_server: 0,
            chat_features: ChatFeatures::default(),
            invitation_link: None,
            invitation_loading: false,
            invitation_error: None,
            input_mode: InputMode::None,
            input: String::new(),
            notice: None,
            area: Cell::new(Rect::default()),
            composer_area: Cell::new(Rect::default()),
            send_area: Cell::new(Rect::default()),
            jump_to_latest_area: Cell::new(Rect::default()),
            max_chat_scroll: Cell::new(0),
            delete_cancel_area: Cell::new(Rect::default()),
            delete_ok_area: Cell::new(Rect::default()),
            chat_deletion_close_area: Cell::new(Rect::default()),
            chat_deletion_change_area: Cell::new(Rect::default()),
            message_hitboxes: RefCell::new(Vec::new()),
            reaction_picker: None,
            reaction_option_areas: RefCell::new(Vec::new()),
            chat_deletion_dialog: None,
            download_cancel_dialog: None,
            download_cancel_no_area: Cell::new(Rect::default()),
            download_cancel_yes_area: Cell::new(Rect::default()),
            events: EventHandler::new(),
            message_cache: HashMap::new(),
            simplex_events,
            simplex_commands,
        }
    }
}

impl App {
    pub const SETTINGS: [&'static str; 7] = [
        "General",
        "Sound",
        "Privacy & Security",
        "Appearance",
        "Chat Features",
        "Servers",
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
        // This is the asynchronous bridge between the TUI and the synchronous
        // SimpleX/Haskell API. Ratatui renders the current `App` state here and
        // Crossterm supplies terminal input, while `simplex_worker` owns the
        // blocking FFI calls on a dedicated OS thread. The two sides communicate
        // exclusively through command/event channels: UI actions enqueue
        // `SimplexCommand`s, and periodic ticks project returned `SimplexEvent`s
        // into `App`. This keeps the terminal responsive and makes this loop the
        // only place from which UI state is mutated.
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
        if self.download_cancel_dialog.is_some() {
            match key.code {
                KeyCode::Char('y') => self.confirm_cancel_download(),
                KeyCode::Enter | KeyCode::Esc | KeyCode::Char('n') => {
                    self.download_cancel_dialog = None
                }
                _ => {}
            }
            return Ok(());
        }
        if self.input_mode == InputMode::ConfirmDeleteProfile {
            match key.code {
                KeyCode::Char('y') => self.confirm_delete_profile(),
                KeyCode::Enter | KeyCode::Esc => self.input_mode = InputMode::None,
                _ => {}
            }
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
                    self.message_cache.clear();
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
        if self.input_mode == InputMode::AddServer {
            match key.code {
                KeyCode::Esc => {
                    self.input_mode = InputMode::None;
                    self.input.clear();
                }
                KeyCode::Enter if !self.input.trim().is_empty() => self.add_server(),
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
        if self.chat_deletion_dialog.is_some() {
            match key.code {
                KeyCode::Esc => self.chat_deletion_dialog = None,
                KeyCode::Enter | KeyCode::Char(' ') => self.cycle_chat_deletion(),
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
            KeyCode::Char('n') if self.section == Section::Chats => self.show_invitation(),
            KeyCode::Char('s')
                if self.section == Section::Chats && self.selected_chat < self.chats.len() =>
            {
                self.open_chat_deletion()
            }
            KeyCode::Char('r')
                if self.section == Section::Chats && self.selected_chat == self.chats.len() =>
            {
                self.create_invitation()
            }
            KeyCode::Enter
                if self.section == Section::Chats && self.selected_chat == self.chats.len() =>
            {
                if self.invitation_link.is_none() {
                    self.create_invitation();
                }
            }
            KeyCode::Enter | KeyCode::Char('i')
                if self.section == Section::Chats && self.selected_chat < self.chats.len() =>
            {
                self.composer_focused = true
            }
            KeyCode::Enter if self.section == Section::Profiles => self.activate_profile(),
            KeyCode::Char('n') if self.section == Section::Profiles => {
                self.input_mode = InputMode::CreateProfile;
                self.input.clear();
            }
            KeyCode::Char('d')
                if self.section == Section::Profiles
                    && self.selected_profile < self.profiles.len() =>
            {
                self.input_mode = InputMode::ConfirmDeleteProfile;
            }
            KeyCode::Enter | KeyCode::Char(' ') if self.section == Section::Settings => {
                if self.selected_setting == 5 {
                    self.toggle_selected_server();
                } else {
                    self.activate_setting()
                }
            }
            KeyCode::Char('a')
                if self.section == Section::Settings && self.selected_setting == 5 =>
            {
                self.input_mode = InputMode::AddServer;
                self.input.clear();
            }
            KeyCode::Char('p')
                if self.section == Section::Settings && self.selected_setting == 5 =>
            {
                self.server_protocol = match self.server_protocol {
                    ServerProtocol::Smp => ServerProtocol::Xftp,
                    ServerProtocol::Xftp => ServerProtocol::Smp,
                };
                self.selected_server = 0;
            }
            KeyCode::Char('k')
                if self.section == Section::Settings && self.selected_setting == 5 =>
            {
                self.selected_server = self.selected_server.saturating_sub(1);
            }
            KeyCode::Char('j')
                if self.section == Section::Settings && self.selected_setting == 5 =>
            {
                self.selected_server = self
                    .selected_server
                    .saturating_add(1)
                    .min(self.visible_servers().len().saturating_sub(1));
            }
            KeyCode::Char('c')
                if self.section == Section::Settings && self.selected_setting == 3 =>
            {
                self.preferences.compact_messages = !self.preferences.compact_messages;
                self.save_preferences();
            }
            KeyCode::Char('x')
                if self.section == Section::Settings && self.selected_setting == 4 =>
            {
                self.toggle_chat_feature(
                    ChatFeature::FullDeletion,
                    !self.chat_features.full_deletion,
                )
            }
            KeyCode::Char('r')
                if self.section == Section::Settings && self.selected_setting == 4 =>
            {
                self.toggle_chat_feature(ChatFeature::Reactions, !self.chat_features.reactions)
            }
            KeyCode::Char('v')
                if self.section == Section::Settings && self.selected_setting == 4 =>
            {
                self.toggle_chat_feature(
                    ChatFeature::VoiceMessages,
                    !self.chat_features.voice_messages,
                )
            }
            KeyCode::Char('f')
                if self.section == Section::Settings && self.selected_setting == 4 =>
            {
                self.toggle_chat_feature(
                    ChatFeature::FilesAndMedia,
                    !self.chat_features.files_and_media,
                )
            }
            KeyCode::Tab => self.events.send(AppEvent::ToggleSection),
            KeyCode::Char('1') => self.events.send(AppEvent::SelectSection(Section::Chats)),
            KeyCode::Char('2') => self.events.send(AppEvent::SelectSection(Section::Profiles)),
            KeyCode::Char('3') => self.events.send(AppEvent::SelectSection(Section::Settings)),
            KeyCode::Up | KeyCode::Char('k') => self.events.send(AppEvent::SelectPrevious),
            KeyCode::Down | KeyCode::Char('j') => self.events.send(AppEvent::SelectNext),
            KeyCode::Left | KeyCode::Char('h') => self.events.send(AppEvent::PreviousSection),
            KeyCode::Right | KeyCode::Char('l') => self.events.send(AppEvent::NextSection),
            KeyCode::PageUp if self.section == Section::Chats => self.scroll_chat_up(5),
            KeyCode::PageDown if self.section == Section::Chats => self.scroll_chat_down(5),
            KeyCode::End if self.section == Section::Chats => self.jump_to_latest(),
            _ => {}
        }
        Ok(())
    }

    fn handle_mouse_event(&mut self, kind: MouseEventKind, column: u16, row: u16) {
        let left_click = matches!(kind, MouseEventKind::Down(MouseButton::Left));
        let button_click = matches!(kind, MouseEventKind::Down(_));
        if button_click && !self.composer_area.get().contains((column, row).into()) {
            self.composer_focused = false;
        }
        if left_click && self.download_cancel_dialog.is_some() {
            if self
                .download_cancel_yes_area
                .get()
                .contains((column, row).into())
            {
                self.confirm_cancel_download();
            } else if self
                .download_cancel_no_area
                .get()
                .contains((column, row).into())
            {
                self.download_cancel_dialog = None;
            }
            return;
        }
        if left_click && self.chat_deletion_dialog.is_some() {
            if self
                .chat_deletion_change_area
                .get()
                .contains((column, row).into())
            {
                self.cycle_chat_deletion();
            } else if self
                .chat_deletion_close_area
                .get()
                .contains((column, row).into())
            {
                self.chat_deletion_dialog = None;
            }
            return;
        }
        if left_click && self.reaction_picker.is_some() {
            let emoji = self
                .reaction_option_areas
                .borrow()
                .iter()
                .find(|(area, _)| area.contains((column, row).into()))
                .map(|(_, emoji)| emoji.clone());
            if let Some(emoji) = emoji {
                self.send_reaction(emoji);
            } else {
                self.reaction_picker = None;
            }
            return;
        }
        if matches!(kind, MouseEventKind::Down(MouseButton::Right)) {
            let item_id = self
                .message_hitboxes
                .borrow()
                .iter()
                .rev()
                .find(|hitbox| hitbox.area.contains((column, row).into()))
                .map(|hitbox| hitbox.item_id);
            if let (Some(chat_ref), Some(item_id)) = (self.selected_chat_ref().cloned(), item_id) {
                self.reaction_picker = Some(ReactionPicker {
                    chat_ref,
                    item_id,
                    column,
                    row,
                });
            } else {
                self.reaction_picker = None;
            }
            return;
        }
        if self.input_mode == InputMode::ConfirmDeleteProfile && left_click {
            if self.delete_ok_area.get().contains((column, row).into()) {
                self.confirm_delete_profile();
            } else if self.delete_cancel_area.get().contains((column, row).into()) {
                self.input_mode = InputMode::None;
            }
            return;
        }
        if left_click {
            let item_id = self
                .message_hitboxes
                .borrow()
                .iter()
                .rev()
                .find(|hitbox| hitbox.area.contains((column, row).into()))
                .map(|hitbox| hitbox.item_id);
            if let Some(item_id) = item_id
                && self.download_attachment(item_id)
            {
                return;
            }
        }
        if left_click {
            if self
                .jump_to_latest_area
                .get()
                .contains((column, row).into())
            {
                self.jump_to_latest();
                return;
            }
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
        if self.section == Section::Chats && column >= area.x + sidebar_width {
            match kind {
                MouseEventKind::ScrollUp => self.scroll_chat_up(3),
                MouseEventKind::ScrollDown => self.scroll_chat_down(3),
                _ => {}
            }
            return;
        }
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
        let previous_section = self.section;
        let previous_chat = self.selected_chat_ref().cloned();
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
        if self.section == Section::Chats
            && (previous_section != Section::Chats
                || previous_chat.as_ref() != self.selected_chat_ref())
        {
            self.jump_to_latest();
            self.loaded_chat = None;
            self.messages.clear();
            self.chat_loading = false;
            self.chat_error = None;
            self.reaction_picker = None;
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
            Section::Chats => self.chats.len() + usize::from(self.active_user().is_some()),
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
                    self.message_cache.clear();
                    self.loaded_chat = None;
                    self.auto_delete_pending = None;
                    self.invitation_link = None;
                    self.invitation_error = None;
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
                    self.message_cache.clear();
                    self.loaded_chat = None;
                    self.auto_delete_pending = None;
                    self.invitation_link = None;
                    self.invitation_error = None;
                    self.notice = Some("Profile created".into());
                    self.sync_selected_profile();
                }
                SimplexEvent::ProfileDeleted {
                    profiles,
                    active_user,
                    chats,
                } => {
                    self.profiles = profiles;
                    self.chats = chats;
                    self.messages.clear();
                    self.message_cache.clear();
                    self.loaded_chat = None;
                    self.auto_delete_pending = None;
                    self.selected_chat = 0;
                    self.startup = active_user
                        .map(StartupState::Ready)
                        .unwrap_or(StartupState::NoActiveUser);
                    self.notice = Some("Profile deleted".into());
                    self.sync_selected_profile();
                }
                SimplexEvent::ProfileDeleteFailed(error) => {
                    self.notice = Some(format!("Could not delete profile: {error}"));
                }
                SimplexEvent::SettingChanged(message) => self.notice = Some(message),
                SimplexEvent::AutoDeleteLoaded(seconds) => {
                    self.auto_delete_seconds = seconds;
                    self.auto_delete_pending = None;
                }
                SimplexEvent::AutoDeleteChanged(seconds) => {
                    self.auto_delete_seconds = seconds;
                    self.auto_delete_pending = None;
                    self.notice = Some("Automatic deletion updated".into());
                }
                SimplexEvent::AutoDeleteFailed(error) => {
                    self.auto_delete_pending = None;
                    self.notice = Some(format!("Could not update automatic deletion: {error}"));
                }
                SimplexEvent::ChatDeletionLoaded { chat_ref, settings } => {
                    if let Some(dialog) = &mut self.chat_deletion_dialog
                        && dialog.chat_ref == chat_ref
                    {
                        dialog.settings = Some(settings);
                        dialog.pending = false;
                        dialog.error = None;
                    }
                }
                SimplexEvent::ChatDeletionChanged { chat_ref, settings } => {
                    if let Some(dialog) = &mut self.chat_deletion_dialog
                        && dialog.chat_ref == chat_ref
                    {
                        dialog.settings = Some(settings);
                        dialog.pending = false;
                        dialog.error = None;
                        self.notice = Some("Chat deletion updated".into());
                    }
                }
                SimplexEvent::ChatDeletionFailed(error) => {
                    if let Some(dialog) = &mut self.chat_deletion_dialog {
                        dialog.pending = false;
                        dialog.error = Some(error);
                    }
                }
                SimplexEvent::FileDownloadStarted { file_id, path } => {
                    update_attachment_status(
                        &mut self.messages,
                        file_id,
                        "rcvTransfer",
                        Some(path.as_str()),
                    );
                    for messages in self.message_cache.values_mut() {
                        update_attachment_status(messages, file_id, "rcvTransfer", Some(&path));
                    }
                    self.notice = Some(format!("Downloading to {path}"));
                }
                SimplexEvent::FileDownloadFailed { file_id, error } => {
                    update_attachment_status(&mut self.messages, file_id, "rcvError", None);
                    for messages in self.message_cache.values_mut() {
                        update_attachment_status(messages, file_id, "rcvError", None);
                    }
                    self.notice = Some(format!("Could not download file: {error}"));
                }
                SimplexEvent::FileDownloadCancelled { file_id } => {
                    update_attachment_status(&mut self.messages, file_id, "rcvCancelled", None);
                    for messages in self.message_cache.values_mut() {
                        update_attachment_status(messages, file_id, "rcvCancelled", None);
                    }
                    self.notice = Some("Download cancelled".into());
                }
                SimplexEvent::FileUpdated { chat_ref, message } => {
                    if self.loaded_chat.as_ref() == Some(&chat_ref) {
                        replace_message(&mut self.messages, message.clone());
                    }
                    if let Some(messages) = self.message_cache.get_mut(&chat_ref) {
                        replace_message(messages, message);
                    }
                }
                SimplexEvent::ServersLoaded(servers) => {
                    self.servers = servers;
                    self.selected_server = self
                        .selected_server
                        .min(self.visible_servers().len().saturating_sub(1));
                }
                SimplexEvent::ServersUpdateFailed(error) => {
                    self.notice = Some(format!("Could not update servers: {error}"));
                }
                SimplexEvent::ChatFeaturesLoaded(features) => self.chat_features = features,
                SimplexEvent::InvitationCreated(link) => {
                    self.invitation_link = Some(link);
                    self.invitation_loading = false;
                    self.invitation_error = None;
                }
                SimplexEvent::InvitationFailed(error) => {
                    self.invitation_loading = false;
                    self.invitation_error = Some(error);
                }
                SimplexEvent::ChatLoaded {
                    chat_ref,
                    mut messages,
                } => {
                    if self.selected_chat_ref() == Some(&chat_ref) {
                        if let Some(cached) = self.message_cache.get(&chat_ref) {
                            for message in cached {
                                if !messages.iter().any(|item| item.id == message.id) {
                                    messages.push(message.clone());
                                }
                            }
                        }
                        messages.sort_by(|left, right| {
                            left.timestamp
                                .cmp(&right.timestamp)
                                .then(left.id.cmp(&right.id))
                        });
                        self.message_cache
                            .insert(chat_ref.clone(), messages.clone());
                        self.loaded_chat = Some(chat_ref);
                        self.chat_loading = false;
                        self.messages = messages;
                        self.chat_error = None;
                        self.chat_scroll = 0;
                        self.new_messages_below = 0;
                    }
                }
                SimplexEvent::ChatMarkedRead(chat_ref) => {
                    if let Some(chat) = self.chats.iter_mut().find(|chat| chat.chat_ref == chat_ref)
                    {
                        chat.unread_count = 0;
                    }
                }
                SimplexEvent::ContactConnected { chats, chat_ref } => {
                    self.chats = chats;
                    self.section = Section::Chats;
                    self.selected_chat = self
                        .chats
                        .iter()
                        .position(|chat| chat.chat_ref == chat_ref)
                        .unwrap_or(0);
                    self.loaded_chat = None;
                    self.messages.clear();
                    self.chat_loading = false;
                    self.chat_error = None;
                    self.invitation_link = None;
                    self.invitation_loading = false;
                    self.invitation_error = None;
                    self.jump_to_latest();
                    self.notice = Some("Contact connected".into());
                }
                SimplexEvent::MessageReceived { chat_ref, message } => {
                    let is_new = !self
                        .message_cache
                        .get(&chat_ref)
                        .is_some_and(|messages| messages.iter().any(|item| item.id == message.id));
                    if !is_new {
                        continue;
                    }
                    if !message.outgoing && self.preferences.notification_sound {
                        ring_terminal_bell();
                    }
                    let chat_is_visible = self.section == Section::Chats
                        && self.selected_chat_ref() == Some(&chat_ref)
                        && self.loaded_chat.as_ref() == Some(&chat_ref)
                        && !self.chat_loading;
                    if chat_is_visible {
                        if !self.messages.iter().any(|item| item.id == message.id) {
                            self.messages.push(message.clone());
                            cache_message(&mut self.message_cache, &chat_ref, message.clone());
                            if !message.outgoing {
                                self.chat_scroll = self.chat_scroll.saturating_add(1);
                                self.new_messages_below = self.new_messages_below.saturating_add(1);
                                if let Some(chat) =
                                    self.chats.iter_mut().find(|chat| chat.chat_ref == chat_ref)
                                {
                                    chat.unread_count = chat.unread_count.saturating_add(1);
                                }
                            } else if self.chat_scroll > 0 {
                                // Preserve the viewport when sending while reading older messages,
                                // without presenting the sent item as a new/unread message.
                                self.chat_scroll = self.chat_scroll.saturating_add(1);
                            }
                        }
                    } else {
                        if !message.outgoing
                            && let Some(chat) =
                                self.chats.iter_mut().find(|chat| chat.chat_ref == chat_ref)
                        {
                            chat.unread_count += 1;
                        }
                        cache_message(&mut self.message_cache, &chat_ref, message);
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
                SimplexEvent::ReactionChanged {
                    chat_ref,
                    item_id,
                    emoji,
                    added,
                    user_reacted,
                } => {
                    if self.loaded_chat.as_ref() == Some(&chat_ref) {
                        update_message_reaction(
                            &mut self.messages,
                            item_id,
                            &emoji,
                            added,
                            user_reacted,
                        );
                    }
                    if let Some(messages) = self.message_cache.get_mut(&chat_ref) {
                        update_message_reaction(messages, item_id, &emoji, added, user_reacted);
                    }
                }
                SimplexEvent::NoActiveUser => self.startup = StartupState::NoActiveUser,
                SimplexEvent::Failed(error) => self.startup = StartupState::Failed(error),
            }
        }
        if self.section == Section::Chats
            && let Some(chat_ref) = self.selected_chat_ref().cloned()
        {
            if self.loaded_chat.as_ref() != Some(&chat_ref) {
                self.loaded_chat = Some(chat_ref.clone());
                self.messages.clear();
                self.chat_loading = true;
                self.chat_error = None;
                let _ = self
                    .simplex_commands
                    .send(SimplexCommand::LoadChat(chat_ref));
            } else if !self.chat_loading && self.chat_error.is_none() && self.chat_scroll == 0 {
                let unread = self
                    .chats
                    .get(self.selected_chat)
                    .is_some_and(|chat| chat.unread_count > 0);
                if unread {
                    if let Some(chat) = self.chats.get_mut(self.selected_chat) {
                        chat.unread_count = 0;
                    }
                    let _ = self
                        .simplex_commands
                        .send(SimplexCommand::MarkChatRead(chat_ref));
                }
            }
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

    fn send_reaction(&mut self, emoji: String) {
        let Some(picker) = self.reaction_picker.take() else {
            return;
        };
        let _ = self.simplex_commands.send(SimplexCommand::SendReaction {
            chat_ref: picker.chat_ref,
            item_id: picker.item_id,
            emoji,
        });
    }

    fn scroll_chat_up(&mut self, amount: usize) {
        let max = self.max_chat_scroll.get();
        self.chat_scroll = self.chat_scroll.saturating_add(amount).min(max);
    }

    fn scroll_chat_down(&mut self, amount: usize) {
        self.chat_scroll = self.chat_scroll.saturating_sub(amount);
        if self.chat_scroll == 0 {
            self.new_messages_below = 0;
        }
    }

    fn jump_to_latest(&mut self) {
        self.chat_scroll = 0;
        self.new_messages_below = 0;
    }

    fn show_invitation(&mut self) {
        if self.active_user().is_none() {
            self.notice = Some("Create a profile first".into());
            return;
        }
        self.selected_chat = self.chats.len();
        if self.invitation_link.is_none() {
            self.create_invitation();
        }
    }

    fn create_invitation(&mut self) {
        let Some(user_id) = self.active_user().map(|user| user.id) else {
            self.notice = Some("Create a profile first".into());
            return;
        };
        if self.invitation_loading {
            return;
        }
        self.invitation_loading = true;
        self.invitation_error = None;
        if self
            .simplex_commands
            .send(SimplexCommand::CreateInvitation { user_id })
            .is_err()
        {
            self.invitation_loading = false;
            self.invitation_error = Some("SimpleX worker is not available".into());
        }
    }

    fn toggle_chat_feature(&mut self, feature: ChatFeature, enabled: bool) {
        let Some(user_id) = self.active_user().map(|user| user.id) else {
            self.notice = Some("Create a profile first".into());
            return;
        };
        self.notice = Some("Updating chat feature…".into());
        let _ = self.simplex_commands.send(SimplexCommand::SetChatFeature {
            user_id,
            feature,
            enabled,
        });
    }

    pub fn visible_servers(&self) -> Vec<&ServerEntry> {
        self.servers
            .iter()
            .filter(|server| server.protocol == self.server_protocol)
            .collect()
    }

    fn toggle_selected_server(&mut self) {
        let Some(user_id) = self.active_user().map(|user| user.id) else {
            self.notice = Some("Create a profile first".into());
            return;
        };
        let Some(server) = self.visible_servers().get(self.selected_server).copied() else {
            self.notice = Some("Add a server first".into());
            return;
        };
        let protocol = server.protocol;
        let address = server.address.clone();
        let enabled = !server.enabled;
        self.notice = Some("Updating server configuration…".into());
        let _ = self
            .simplex_commands
            .send(SimplexCommand::SetServerEnabled {
                user_id,
                protocol,
                address,
                enabled,
            });
    }

    fn add_server(&mut self) {
        let Some(user_id) = self.active_user().map(|user| user.id) else {
            self.notice = Some("Create a profile first".into());
            return;
        };
        let address = self.input.trim().to_owned();
        self.input_mode = InputMode::None;
        self.input.clear();
        self.notice = Some(format!("Adding {} server…", self.server_protocol.label()));
        let _ = self.simplex_commands.send(SimplexCommand::AddServer {
            user_id,
            protocol: self.server_protocol,
            address,
        });
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

    fn confirm_delete_profile(&mut self) {
        let Some(profile) = self.profiles.get(self.selected_profile) else {
            self.input_mode = InputMode::None;
            return;
        };
        let user_id = profile.id;
        self.input_mode = InputMode::None;
        self.notice = Some(format!("Deleting profile {}…", profile.display_name));
        let _ = self
            .simplex_commands
            .send(SimplexCommand::DeleteProfile(user_id));
    }

    fn open_chat_deletion(&mut self) {
        let Some(chat_ref) = self.selected_chat_ref().cloned() else {
            return;
        };
        self.chat_deletion_dialog = Some(ChatDeletionDialog {
            chat_ref: chat_ref.clone(),
            settings: None,
            pending: true,
            error: None,
        });
        let _ = self
            .simplex_commands
            .send(SimplexCommand::LoadChatDeletion { chat_ref });
    }

    fn download_attachment(&mut self, item_id: i64) -> bool {
        let Some(message) = self.messages.iter().find(|message| message.id == item_id) else {
            return false;
        };
        if message.outgoing {
            return false;
        }
        let Some(attachment) = message.attachment.as_ref() else {
            return false;
        };
        if attachment.status == "rcvComplete" {
            self.notice = attachment
                .path
                .as_ref()
                .map(|path| format!("File saved to {path}"));
            return true;
        }
        if matches!(attachment.status.as_str(), "rcvAccepted" | "rcvTransfer") {
            self.download_cancel_dialog = Some(DownloadCancelDialog {
                file_id: attachment.id,
                file_name: attachment.name.clone(),
            });
            return true;
        }
        let _ = self.simplex_commands.send(SimplexCommand::ReceiveFile {
            file_id: attachment.id,
            file_name: attachment.name.clone(),
        });
        self.notice = Some("Starting file download…".into());
        true
    }

    fn confirm_cancel_download(&mut self) {
        let Some(dialog) = self.download_cancel_dialog.take() else {
            return;
        };
        let _ = self.simplex_commands.send(SimplexCommand::CancelFile {
            file_id: dialog.file_id,
        });
        self.notice = Some(format!("Cancelling download of {}…", dialog.file_name));
    }

    fn cycle_chat_deletion(&mut self) {
        let Some(user_id) = self.active_user().map(|user| user.id) else {
            return;
        };
        let Some(dialog) = &mut self.chat_deletion_dialog else {
            return;
        };
        if dialog.pending || dialog.settings.is_none() {
            return;
        }
        let settings = dialog.settings.as_ref().expect("checked above");
        let current = (settings.local_ttl == settings.disappearing_ttl)
            .then_some(settings.local_ttl)
            .flatten();
        let seconds = match current {
            None => Some(0),
            Some(0) => Some(86_400),
            Some(86_400) => Some(604_800),
            Some(604_800) => Some(2_592_000),
            Some(2_592_000) => None,
            Some(_) => Some(0),
        };
        dialog.pending = true;
        dialog.error = None;
        let _ = self.simplex_commands.send(SimplexCommand::SetChatDeletion {
            user_id,
            chat_ref: dialog.chat_ref.clone(),
            seconds,
        });
    }

    fn activate_setting(&mut self) {
        match self.selected_setting {
            1 => {
                self.preferences.notification_sound = !self.preferences.notification_sound;
                self.save_preferences();
                if self.preferences.notification_sound {
                    ring_terminal_bell();
                }
            }
            2 => {
                if self.auto_delete_pending.is_some() {
                    self.notice = Some("Automatic deletion update is still pending".into());
                    return;
                }
                let Some(user_id) = self.active_user().map(|user| user.id) else {
                    self.notice = Some("Create a profile first".into());
                    return;
                };
                let seconds = match self.auto_delete_seconds {
                    0 => 86_400,
                    86_400 => 604_800,
                    604_800 => 2_592_000,
                    _ => 0,
                };
                if self
                    .simplex_commands
                    .send(SimplexCommand::SetAutoDelete { user_id, seconds })
                    .is_ok()
                {
                    self.auto_delete_pending = Some(seconds);
                    self.notice = Some("Updating automatic deletion…".into());
                } else {
                    self.notice = Some("SimpleX worker is not available".into());
                }
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

fn cache_message(cache: &mut HashMap<ChatRef, Vec<Message>>, chat_ref: &ChatRef, message: Message) {
    let messages = cache.entry(chat_ref.clone()).or_default();
    if !messages.iter().any(|item| item.id == message.id) {
        messages.push(message);
        messages.sort_by(|left, right| {
            left.timestamp
                .cmp(&right.timestamp)
                .then(left.id.cmp(&right.id))
        });
    }
}

fn update_message_reaction(
    messages: &mut [Message],
    item_id: i64,
    emoji: &str,
    added: bool,
    reacted_by_user: bool,
) {
    let Some(message) = messages.iter_mut().find(|message| message.id == item_id) else {
        return;
    };
    if let Some(reaction) = message
        .reactions
        .iter_mut()
        .find(|reaction| reaction.emoji == emoji)
    {
        reaction.count = if added {
            reaction.count.saturating_add(1)
        } else {
            reaction.count.saturating_sub(1)
        };
        if reacted_by_user {
            reaction.user_reacted = added;
        }
    } else if added {
        message.reactions.push(crate::chat::MessageReaction {
            emoji: emoji.to_owned(),
            count: 1,
            user_reacted: reacted_by_user,
        });
    }
    message.reactions.retain(|reaction| reaction.count > 0);
}

fn update_attachment_status(
    messages: &mut [Message],
    file_id: i64,
    status: &str,
    path: Option<&str>,
) {
    if let Some(attachment) = messages
        .iter_mut()
        .filter_map(|message| message.attachment.as_mut())
        .find(|attachment| attachment.id == file_id)
    {
        attachment.status = status.into();
        if let Some(path) = path {
            attachment.path = Some(path.into());
        }
    }
}

fn replace_message(messages: &mut [Message], updated: Message) {
    if let Some(message) = messages.iter_mut().find(|message| message.id == updated.id) {
        *message = updated;
    }
}

fn ring_terminal_bell() {
    let mut stdout = io::stdout();
    let _ = stdout.write_all(b"\x07");
    let _ = stdout.flush();
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
        app.composer_area.set(Rect::new(35, 20, 50, 5));
        app.composer_focused = true;

        app.handle_mouse_event(MouseEventKind::Down(MouseButton::Left), 12, 1);
        assert_eq!(app.section, Section::Profiles);
        assert!(!app.composer_focused);

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

    #[tokio::test]
    async fn incoming_message_only_clears_when_its_chat_is_visible() {
        let (event_sender, event_receiver) = mpsc::channel();
        let mut app = App {
            section: Section::Settings,
            chats: vec![ChatSummary {
                chat_ref: ChatRef("@7".into()),
                display_name: "alice".into(),
                unread_count: 0,
            }],
            loaded_chat: Some(ChatRef("@7".into())),
            simplex_events: event_receiver,
            ..App::default()
        };
        event_sender
            .send(SimplexEvent::MessageReceived {
                chat_ref: ChatRef("@7".into()),
                message: Message {
                    id: 1,
                    text: "hello".into(),
                    timestamp: String::new(),
                    outgoing: false,
                    reactions: Vec::new(),
                    attachment: None,
                },
            })
            .unwrap();

        app.tick();

        assert_eq!(app.chats[0].unread_count, 1);
        assert!(app.messages.is_empty());

        app.handle_app_event(AppEvent::SelectSection(Section::Chats));
        assert_eq!(app.loaded_chat, None);
        event_sender
            .send(SimplexEvent::ChatLoaded {
                chat_ref: ChatRef("@7".into()),
                messages: Vec::new(),
            })
            .unwrap();
        app.tick();
        assert_eq!(app.messages[0].text, "hello");
    }

    #[tokio::test]
    async fn outgoing_message_is_shown_without_becoming_unread() {
        let (event_sender, event_receiver) = mpsc::channel();
        let chat_ref = ChatRef("@7".into());
        let mut app = App {
            section: Section::Chats,
            chats: vec![ChatSummary {
                chat_ref: chat_ref.clone(),
                display_name: "alice".into(),
                unread_count: 0,
            }],
            loaded_chat: Some(chat_ref.clone()),
            simplex_events: event_receiver,
            ..App::default()
        };
        event_sender
            .send(SimplexEvent::MessageReceived {
                chat_ref,
                message: Message {
                    id: 1,
                    text: "sent from the TUI".into(),
                    timestamp: String::new(),
                    outgoing: true,
                    reactions: Vec::new(),
                    attachment: None,
                },
            })
            .unwrap();

        app.tick();

        assert_eq!(app.messages.len(), 1);
        assert_eq!(app.chats[0].unread_count, 0);
        assert_eq!(app.new_messages_below, 0);
        assert_eq!(app.chat_scroll, 0);
    }

    #[tokio::test]
    async fn outgoing_background_message_does_not_increment_unread_count() {
        let (event_sender, event_receiver) = mpsc::channel();
        let mut app = App {
            section: Section::Settings,
            chats: vec![ChatSummary {
                chat_ref: ChatRef("@7".into()),
                display_name: "alice".into(),
                unread_count: 0,
            }],
            simplex_events: event_receiver,
            ..App::default()
        };
        event_sender
            .send(SimplexEvent::MessageReceived {
                chat_ref: ChatRef("@7".into()),
                message: Message {
                    id: 2,
                    text: "sent elsewhere".into(),
                    timestamp: String::new(),
                    outgoing: true,
                    reactions: Vec::new(),
                    attachment: None,
                },
            })
            .unwrap();

        app.tick();

        assert_eq!(app.chats[0].unread_count, 0);
    }

    #[tokio::test]
    async fn returning_to_an_already_loaded_chat_marks_it_read() {
        let (commands, command_receiver) = mpsc::channel();
        let mut app = App {
            section: Section::Chats,
            chats: vec![ChatSummary {
                chat_ref: ChatRef("@7".into()),
                display_name: "alice".into(),
                unread_count: 2,
            }],
            loaded_chat: Some(ChatRef("@7".into())),
            simplex_commands: commands,
            ..App::default()
        };

        app.tick();

        assert_eq!(app.chats[0].unread_count, 0);
        let SimplexCommand::MarkChatRead(chat_ref) = command_receiver.try_recv().unwrap() else {
            panic!("expected mark-read command")
        };
        assert_eq!(chat_ref, ChatRef("@7".into()));
    }

    #[tokio::test]
    async fn message_received_in_background_is_merged_when_chat_opens() {
        let (event_sender, event_receiver) = mpsc::channel();
        let mut app = App {
            section: Section::Chats,
            selected_chat: 0,
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
            loaded_chat: Some(ChatRef("@1".into())),
            simplex_events: event_receiver,
            ..App::default()
        };
        let message = Message {
            id: 42,
            text: "from background".into(),
            timestamp: String::new(),
            outgoing: false,
            reactions: Vec::new(),
            attachment: None,
        };
        event_sender
            .send(SimplexEvent::MessageReceived {
                chat_ref: ChatRef("@2".into()),
                message: message.clone(),
            })
            .unwrap();
        app.tick();
        assert_eq!(app.chats[1].unread_count, 1);

        app.selected_chat = 1;
        event_sender
            .send(SimplexEvent::ChatLoaded {
                chat_ref: ChatRef("@2".into()),
                messages: Vec::new(),
            })
            .unwrap();
        app.tick();

        assert_eq!(app.messages, vec![message]);

        event_sender
            .send(SimplexEvent::ChatLoaded {
                chat_ref: ChatRef("@2".into()),
                messages: Vec::new(),
            })
            .unwrap();
        app.tick();
        assert_eq!(app.messages[0].text, "from background");
    }

    #[tokio::test]
    async fn profile_delete_requires_explicit_confirmation() {
        let (commands, receiver) = mpsc::channel();
        let mut app = App {
            section: Section::Profiles,
            profiles: vec![Profile {
                id: 9,
                display_name: "work".into(),
                notifications: true,
                active: true,
            }],
            simplex_commands: commands,
            ..App::default()
        };

        app.handle_key_events(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.input_mode, InputMode::ConfirmDeleteProfile);
        app.handle_key_events(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.input_mode, InputMode::None);
        assert!(receiver.try_recv().is_err());

        app.handle_key_events(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE))
            .unwrap();
        app.handle_key_events(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE))
            .unwrap();
        let SimplexCommand::DeleteProfile(user_id) = receiver.try_recv().unwrap() else {
            panic!("expected delete-profile command")
        };
        assert_eq!(user_id, 9);
    }

    #[tokio::test]
    async fn automatic_deletion_changes_only_after_core_confirmation() {
        let (commands, receiver) = mpsc::channel();
        let (event_sender, event_receiver) = mpsc::channel();
        let mut app = App {
            section: Section::Settings,
            selected_setting: 2,
            startup: StartupState::Ready(User {
                id: 3,
                display_name: "alice".into(),
                notifications: true,
                active: true,
            }),
            simplex_commands: commands,
            simplex_events: event_receiver,
            ..App::default()
        };

        app.activate_setting();
        assert_eq!(app.auto_delete_seconds, 0);
        assert_eq!(app.auto_delete_pending, Some(86_400));
        let SimplexCommand::SetAutoDelete { user_id, seconds } = receiver.try_recv().unwrap()
        else {
            panic!("expected auto-delete command")
        };
        assert_eq!((user_id, seconds), (3, 86_400));

        event_sender
            .send(SimplexEvent::AutoDeleteChanged(86_400))
            .unwrap();
        app.tick();
        assert_eq!(app.auto_delete_seconds, 86_400);
        assert_eq!(app.auto_delete_pending, None);
    }

    #[tokio::test]
    async fn chat_deletion_uses_a_combined_per_chat_override() {
        let (commands, receiver) = mpsc::channel();
        let (event_sender, event_receiver) = mpsc::channel();
        let chat_ref = ChatRef("@7".into());
        let mut app = App {
            section: Section::Chats,
            chats: vec![ChatSummary {
                chat_ref: chat_ref.clone(),
                display_name: "bob".into(),
                unread_count: 0,
            }],
            startup: StartupState::Ready(User {
                id: 3,
                display_name: "alice".into(),
                notifications: true,
                active: true,
            }),
            simplex_commands: commands,
            simplex_events: event_receiver,
            ..App::default()
        };

        app.handle_key_events(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE))
            .unwrap();
        let SimplexCommand::LoadChatDeletion { chat_ref: loaded } = receiver.try_recv().unwrap()
        else {
            panic!("expected chat-deletion load command")
        };
        assert_eq!(loaded, chat_ref);

        event_sender
            .send(SimplexEvent::ChatDeletionLoaded {
                chat_ref: chat_ref.clone(),
                settings: ChatDeletionSettings::default(),
            })
            .unwrap();
        app.tick();
        app.handle_key_events(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        let (user_id, changed, seconds) = loop {
            match receiver.try_recv().unwrap() {
                SimplexCommand::SetChatDeletion {
                    user_id,
                    chat_ref,
                    seconds,
                } => break (user_id, chat_ref, seconds),
                SimplexCommand::LoadChat(_) => {}
                command => panic!("unexpected command: {command:?}"),
            }
        };
        assert_eq!((user_id, changed, seconds), (3, chat_ref, Some(0)));
        assert!(app.chat_deletion_dialog.as_ref().unwrap().pending);
    }

    #[tokio::test]
    async fn download_cancel_defaults_to_no_and_requires_confirmation() {
        let (commands, receiver) = mpsc::channel();
        let mut app = App {
            messages: vec![Message {
                id: 8,
                text: String::new(),
                timestamp: String::new(),
                outgoing: false,
                reactions: Vec::new(),
                attachment: Some(crate::chat::Attachment {
                    id: 41,
                    name: "archive.zip".into(),
                    size: 100,
                    kind: crate::chat::AttachmentKind::File,
                    status: "rcvTransfer".into(),
                    progress: Some(50),
                    path: None,
                }),
            }],
            simplex_commands: commands,
            ..App::default()
        };

        assert!(app.download_attachment(8));
        app.handle_key_events(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert!(app.download_cancel_dialog.is_none());
        assert!(receiver.try_recv().is_err());

        assert!(app.download_attachment(8));
        app.handle_key_events(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE))
            .unwrap();
        let SimplexCommand::CancelFile { file_id } = receiver.try_recv().unwrap() else {
            panic!("expected cancel-file command")
        };
        assert_eq!(file_id, 41);
    }

    #[tokio::test]
    async fn server_settings_toggle_and_add_both_protocols() {
        let (commands, receiver) = mpsc::channel();
        let mut app = App {
            section: Section::Settings,
            selected_setting: 5,
            startup: StartupState::Ready(User {
                id: 3,
                display_name: "alice".into(),
                notifications: true,
                active: true,
            }),
            servers: vec![ServerEntry {
                protocol: ServerProtocol::Smp,
                address: "smp://key@smp.example".into(),
                enabled: true,
                preset: true,
            }],
            simplex_commands: commands,
            ..App::default()
        };

        app.handle_key_events(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        let SimplexCommand::SetServerEnabled {
            user_id,
            protocol,
            address,
            enabled,
        } = receiver.try_recv().unwrap()
        else {
            panic!("expected server-toggle command")
        };
        assert_eq!(user_id, 3);
        assert_eq!(protocol, ServerProtocol::Smp);
        assert_eq!(address, "smp://key@smp.example");
        assert!(!enabled);

        app.handle_key_events(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.server_protocol, ServerProtocol::Xftp);
        app.handle_key_events(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
            .unwrap();
        app.input = "xftp://key@files.example".into();
        app.handle_key_events(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        let SimplexCommand::AddServer {
            user_id,
            protocol,
            address,
        } = receiver.try_recv().unwrap()
        else {
            panic!("expected add-server command")
        };
        assert_eq!((user_id, protocol), (3, ServerProtocol::Xftp));
        assert_eq!(address, "xftp://key@files.example");
    }
}
