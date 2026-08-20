use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    io::{self, Write},
    path::PathBuf,
    sync::mpsc,
};

use crate::event::{AppEvent, Event, EventHandler};
use crate::preferences::Preferences;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use libsimplex_rs::{
    ChatDeleteMode, ChatDeletionSettings, ChatFeature, ChatFeatures, ChatRef, ChatSummary,
    Command as SimplexCommand, Event as SimplexEvent, GroupMember, Message, Profile, ServerEntry,
    ServerProtocol, Session, User,
};
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
    RenameProfile,
    ConfirmDeleteProfile,
    AddServer,
    ConnectInvitation,
    CreateGroup,
    RenameGroup,
}

#[derive(Clone, Debug)]
pub(crate) struct GroupManagementDialog {
    pub chat_ref: ChatRef,
    pub members: Vec<GroupMember>,
    pub selected: usize,
    pub adding: bool,
    pub role_target: Option<usize>,
    pub pending: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) enum GroupAction {
    Remove {
        member_id: i64,
        name: String,
    },
    Block {
        member_id: i64,
        name: String,
        blocked: bool,
    },
    ChangeRole {
        member_id: i64,
        name: String,
        role: String,
    },
    Leave,
    Delete,
    DeleteLocal,
}

#[derive(Clone, Debug)]
pub(crate) struct GroupConfirmation {
    pub chat_ref: ChatRef,
    pub group_name: String,
    pub action: GroupAction,
}

#[derive(Clone, Debug)]
pub(crate) struct MessageHitbox {
    pub area: Rect,
    pub item_id: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct GroupInvitationHitbox {
    pub area: Rect,
    pub item_id: i64,
    pub group_id: i64,
    pub accept: bool,
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
    pub features: Option<ChatFeatures>,
    pub selected: usize,
    pub pending: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct ChatDeleteConfirmation {
    pub chat_ref: ChatRef,
    pub chat_name: String,
    pub mode: ChatDeleteMode,
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
    pub connection_loading: bool,
    pub connection_error: Option<String>,
    pub input_mode: InputMode,
    pub input: String,
    pub profile_create_pending: bool,
    pub notice: Option<String>,
    data_directory: PathBuf,
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
    pub(crate) chat_setting_areas: RefCell<Vec<(Rect, usize)>>,
    pub(crate) message_hitboxes: RefCell<Vec<MessageHitbox>>,
    pub(crate) group_invitation_hitboxes: RefCell<Vec<GroupInvitationHitbox>>,
    pub(crate) reaction_picker: Option<ReactionPicker>,
    pub(crate) reaction_option_areas: RefCell<Vec<(Rect, String)>>,
    pub(crate) chat_deletion_dialog: Option<ChatDeletionDialog>,
    pub(crate) chat_delete_confirmation: Option<ChatDeleteConfirmation>,
    pub(crate) download_cancel_dialog: Option<DownloadCancelDialog>,
    pub(crate) group_management_dialog: Option<GroupManagementDialog>,
    pub(crate) group_confirmation: Option<GroupConfirmation>,
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
            connection_loading: false,
            connection_error: None,
            input_mode: InputMode::None,
            input: String::new(),
            profile_create_pending: false,
            notice: None,
            data_directory: PathBuf::new(),
            area: Cell::new(Rect::default()),
            composer_area: Cell::new(Rect::default()),
            send_area: Cell::new(Rect::default()),
            jump_to_latest_area: Cell::new(Rect::default()),
            max_chat_scroll: Cell::new(0),
            delete_cancel_area: Cell::new(Rect::default()),
            delete_ok_area: Cell::new(Rect::default()),
            chat_deletion_close_area: Cell::new(Rect::default()),
            chat_deletion_change_area: Cell::new(Rect::default()),
            chat_setting_areas: RefCell::new(Vec::new()),
            message_hitboxes: RefCell::new(Vec::new()),
            group_invitation_hitboxes: RefCell::new(Vec::new()),
            reaction_picker: None,
            reaction_option_areas: RefCell::new(Vec::new()),
            chat_deletion_dialog: None,
            chat_delete_confirmation: None,
            download_cancel_dialog: None,
            group_management_dialog: None,
            group_confirmation: None,
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

    pub fn new(session: Session, data_directory: PathBuf) -> Self {
        let (commands, events) = session.into_parts();
        let preferences = Preferences::load(&data_directory);

        Self {
            preferences,
            data_directory,
            simplex_events: events,
            simplex_commands: commands,
            ..Self::default()
        }
    }

    pub async fn run(mut self, mut terminal: DefaultTerminal) -> color_eyre::Result<()> {
        // This is the asynchronous bridge between the TUI and the synchronous
        // SimpleX/Haskell API. Ratatui renders the current `App` state here and
        // Crossterm supplies terminal input, while `libsimplex-rs` owns the
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
                    crossterm::event::Event::Paste(text) => self.handle_paste(text),
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
        if self.group_confirmation.is_some() {
            match key.code {
                KeyCode::Char('y') => self.confirm_group_action(),
                KeyCode::Enter | KeyCode::Esc | KeyCode::Char('n') => {
                    self.group_confirmation = None
                }
                _ => {}
            }
            return Ok(());
        }
        if self.chat_delete_confirmation.is_some() {
            match key.code {
                KeyCode::Char('y') => self.confirm_chat_delete(),
                KeyCode::Enter | KeyCode::Esc | KeyCode::Char('n') => {
                    self.chat_delete_confirmation = None
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
            if self.profile_create_pending {
                return Ok(());
            }
            match key.code {
                KeyCode::Esc => {
                    self.input_mode = InputMode::None;
                    self.input.clear();
                }
                KeyCode::Enter if !self.input.trim().is_empty() => {
                    let name = self.input.trim().to_owned();
                    self.notice = Some("Creating profile…".into());
                    self.profile_create_pending = true;
                    if self
                        .simplex_commands
                        .send(SimplexCommand::CreateProfile(name))
                        .is_err()
                    {
                        self.profile_create_pending = false;
                        self.notice = Some("SimpleX worker is not available".into());
                    }
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
        if self.input_mode == InputMode::RenameProfile {
            match key.code {
                KeyCode::Esc => {
                    self.input_mode = InputMode::None;
                    self.input.clear();
                }
                KeyCode::Enter if !self.input.trim().is_empty() => self.rename_profile(),
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
        if self.input_mode == InputMode::ConnectInvitation {
            match key.code {
                KeyCode::Esc => {
                    self.input_mode = InputMode::None;
                    self.input.clear();
                }
                KeyCode::Enter if !self.input.trim().is_empty() => self.connect_invitation(),
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
        if self.input_mode == InputMode::CreateGroup {
            match key.code {
                KeyCode::Esc => {
                    self.input_mode = InputMode::None;
                    self.input.clear();
                }
                KeyCode::Enter if !self.input.trim().is_empty() => self.create_group(),
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
        if self.input_mode == InputMode::RenameGroup {
            match key.code {
                KeyCode::Esc => {
                    self.input_mode = InputMode::None;
                    self.input.clear();
                }
                KeyCode::Enter if !self.input.trim().is_empty() => self.rename_group(),
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
        if self.group_management_dialog.is_some() {
            self.handle_group_management_key(key);
            return Ok(());
        }
        if self.chat_deletion_dialog.is_some() {
            match key.code {
                KeyCode::Esc => self.chat_deletion_dialog = None,
                KeyCode::Up | KeyCode::Char('k') => self.select_chat_setting_up(),
                KeyCode::Down | KeyCode::Char('j') => self.select_chat_setting_down(),
                KeyCode::Enter | KeyCode::Char(' ') => self.activate_chat_setting(),
                _ => {}
            }
            return Ok(());
        }
        if self.composer_focused {
            if !self.selected_chat_is_writable() {
                self.composer_focused = false;
                self.composer.clear();
                return Ok(());
            }
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
        // The final Profiles row is the create-profile editor itself.  Start
        // editing as soon as the user types, instead of silently discarding
        // the first name characters until Enter or `n` was pressed.
        if self.section == Section::Profiles && self.selected_profile >= self.profiles.len() {
            match key.code {
                KeyCode::Enter => {
                    self.input_mode = InputMode::CreateProfile;
                    self.input.clear();
                }
                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.input_mode = InputMode::CreateProfile;
                    self.input.clear();
                    self.input.push(character);
                }
                _ => {}
            }
            return Ok(());
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.events.send(AppEvent::Quit),
            KeyCode::Char('n') if self.section == Section::Chats => self.show_invitation(),
            KeyCode::Char('p') if self.section == Section::Chats => self.show_connect_invitation(),
            KeyCode::Char('g') if self.section == Section::Chats => {
                self.input_mode = InputMode::CreateGroup;
                self.input.clear();
            }
            KeyCode::Char('m')
                if self.section == Section::Chats
                    && self
                        .selected_chat_ref()
                        .is_some_and(|chat| chat.0.starts_with('#')) =>
            {
                self.open_group_management();
            }
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
                if self.section == Section::Chats
                    && self.selected_chat < self.chats.len()
                    && self.selected_chat_is_writable() =>
            {
                self.composer_focused = true
            }
            KeyCode::Enter if self.section == Section::Profiles => self.activate_profile(),
            KeyCode::Char('n') if self.section == Section::Profiles => {
                self.input_mode = InputMode::CreateProfile;
                self.input.clear();
            }
            KeyCode::Char('r')
                if self.section == Section::Profiles
                    && self.selected_profile < self.profiles.len() =>
            {
                self.input_mode = InputMode::RenameProfile;
                self.input = self.profiles[self.selected_profile].display_name.clone();
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
        if left_click && self.chat_delete_confirmation.is_some() {
            if self
                .chat_deletion_change_area
                .get()
                .contains((column, row).into())
            {
                self.confirm_chat_delete();
            } else if self
                .chat_deletion_close_area
                .get()
                .contains((column, row).into())
            {
                self.chat_delete_confirmation = None;
            }
            return;
        }
        if left_click && self.chat_deletion_dialog.is_some() {
            let selected = self
                .chat_setting_areas
                .borrow()
                .iter()
                .find(|(area, _)| area.contains((column, row).into()))
                .map(|(_, index)| *index);
            if let Some(selected) = selected {
                if let Some(dialog) = &mut self.chat_deletion_dialog {
                    dialog.selected = selected;
                }
                self.activate_chat_setting();
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
        if left_click {
            let invitation = self
                .group_invitation_hitboxes
                .borrow()
                .iter()
                .find(|hitbox| hitbox.area.contains((column, row).into()))
                .cloned();
            if let Some(invitation) = invitation {
                self.answer_group_invitation(
                    invitation.item_id,
                    invitation.group_id,
                    invitation.accept,
                );
                return;
            }
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
                self.composer_focused = self.selected_chat_is_writable();
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
        if previous_section != self.section
            && matches!(
                self.input_mode,
                InputMode::CreateProfile
                    | InputMode::RenameProfile
                    | InputMode::AddServer
                    | InputMode::ConnectInvitation
                    | InputMode::CreateGroup
                    | InputMode::RenameGroup
            )
        {
            self.input_mode = InputMode::None;
            self.input.clear();
        }
        if previous_section != self.section {
            self.group_management_dialog = None;
            self.group_confirmation = None;
        }
        // Clicking the create row (or opening Profiles before the first
        // profile exists) must focus the visible input immediately.
        if self.section == Section::Profiles
            && self.selected_profile >= self.profiles.len()
            && (previous_section != self.section || matches!(event, AppEvent::SelectIndex(_)))
        {
            self.input_mode = InputMode::CreateProfile;
            self.input.clear();
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
                    self.profile_create_pending = false;
                    self.input_mode = InputMode::None;
                    self.input.clear();
                    self.invitation_link = None;
                    self.invitation_error = None;
                    self.notice = Some("Profile created".into());
                    self.sync_selected_profile();
                }
                SimplexEvent::ProfileCreateFailed(error) => {
                    self.profile_create_pending = false;
                    self.notice = Some(format!("Could not create profile: {error}"));
                }
                SimplexEvent::ProfileRenamed {
                    profiles,
                    active_user,
                } => {
                    self.profiles = profiles;
                    if let Some(user) = active_user {
                        self.startup = StartupState::Ready(user);
                    }
                    self.input_mode = InputMode::None;
                    self.input.clear();
                    self.notice = Some("Profile renamed".into());
                    self.sync_selected_profile();
                }
                SimplexEvent::ProfileRenameFailed(error) => {
                    self.notice = Some(format!("Could not rename profile: {error}"));
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
                SimplexEvent::ConversationFeaturesLoaded { chat_ref, features }
                | SimplexEvent::ConversationFeaturesChanged { chat_ref, features } => {
                    if let Some(dialog) = &mut self.chat_deletion_dialog
                        && dialog.chat_ref == chat_ref
                    {
                        dialog.features = Some(features);
                        dialog.pending = false;
                        dialog.error = None;
                    }
                }
                SimplexEvent::ConversationFeaturesFailed(error) => {
                    if let Some(dialog) = &mut self.chat_deletion_dialog {
                        dialog.pending = false;
                        dialog.error = Some(error);
                    }
                }
                SimplexEvent::ChatDeleted { chat_ref, chats } => {
                    self.chats = chats;
                    self.message_cache.remove(&chat_ref);
                    self.selected_chat = self.selected_chat.min(self.chats.len().saturating_sub(1));
                    self.loaded_chat = None;
                    self.messages.clear();
                    self.chat_deletion_dialog = None;
                    self.chat_delete_confirmation = None;
                    self.notice = Some("Chat updated".into());
                }
                SimplexEvent::ChatDeleteFailed(error) => {
                    self.chat_delete_confirmation = None;
                    if let Some(dialog) = &mut self.chat_deletion_dialog {
                        dialog.pending = false;
                        dialog.error = Some(error);
                    } else {
                        self.notice = Some(format!("Could not delete chat: {error}"));
                    }
                }
                SimplexEvent::GroupCreated { chat_ref, chats } => {
                    self.chats = chats;
                    self.input_mode = InputMode::None;
                    self.input.clear();
                    self.selected_chat = self
                        .chats
                        .iter()
                        .position(|chat| chat.chat_ref == chat_ref)
                        .unwrap_or(0);
                    self.notice = Some("Group created".into());
                    self.loaded_chat = None;
                }
                SimplexEvent::GroupMembersLoaded { chat_ref, members } => {
                    if let Some(dialog) = &mut self.group_management_dialog
                        && dialog.chat_ref == chat_ref
                    {
                        dialog.members = members;
                        dialog.pending = false;
                        dialog.error = None;
                        dialog.selected =
                            dialog.selected.min(dialog.members.len().saturating_sub(1));
                    }
                }
                SimplexEvent::GroupChanged {
                    chat_ref,
                    chats,
                    members,
                    message,
                } => {
                    let group_closed = matches!(message.as_str(), "Group left" | "Group deleted");
                    if !chats.is_empty() {
                        self.chats = chats;
                        self.selected_chat = self.selected_chat.min(self.chats.len());
                    }
                    if let Some(dialog) = &mut self.group_management_dialog
                        && dialog.chat_ref == chat_ref
                    {
                        dialog.members = members;
                        dialog.pending = false;
                        dialog.adding = false;
                        dialog.role_target = None;
                        dialog.error = None;
                    }
                    self.notice = Some(message);
                    if !self.selected_chat_is_writable() {
                        self.composer_focused = false;
                        self.composer.clear();
                    }
                    if group_closed || !self.chats.iter().any(|chat| chat.chat_ref == chat_ref) {
                        self.group_management_dialog = None;
                        if !self.chats.iter().any(|chat| chat.chat_ref == chat_ref) {
                            self.loaded_chat = None;
                            self.messages.clear();
                        }
                    }
                }
                SimplexEvent::GroupActionFailed(error) => {
                    if let Some(dialog) = &mut self.group_management_dialog {
                        dialog.pending = false;
                        dialog.error = Some(error.clone());
                    }
                    self.notice = Some(format!("Group action failed: {error}"));
                }
                SimplexEvent::GroupInvitationAnswered {
                    contact_chat_ref,
                    item_id,
                    chats,
                    accepted,
                } => {
                    self.chats = chats;
                    if let Some(messages) = self.message_cache.get_mut(&contact_chat_ref) {
                        messages.retain(|message| message.id != item_id);
                    }
                    if self.loaded_chat.as_ref() == Some(&contact_chat_ref) {
                        self.messages.retain(|message| message.id != item_id);
                    }
                    self.notice = Some(
                        if accepted {
                            "Group joined"
                        } else {
                            "Group invitation declined"
                        }
                        .into(),
                    );
                }
                SimplexEvent::GroupListUpdated(chats) => {
                    let selected = self.selected_chat_ref().cloned();
                    self.chats = chats;
                    let selected_position = selected.as_ref().and_then(|selected| {
                        self.chats
                            .iter()
                            .position(|chat| &chat.chat_ref == selected)
                    });
                    self.selected_chat = selected_position.unwrap_or_else(|| {
                        self.selected_chat.min(self.chats.len().saturating_sub(1))
                    });
                    if let Some(removed) = selected
                        && !self.chats.iter().any(|chat| chat.chat_ref == removed)
                    {
                        self.message_cache.remove(&removed);
                        if self.loaded_chat.as_ref() == Some(&removed) {
                            self.loaded_chat = None;
                            self.messages.clear();
                        }
                        self.group_management_dialog = None;
                    }
                    if !self.selected_chat_is_writable() {
                        self.composer_focused = false;
                        self.composer.clear();
                    }
                    if let Some(chat_ref) = self
                        .group_management_dialog
                        .as_ref()
                        .map(|dialog| dialog.chat_ref.clone())
                    {
                        let _ = self
                            .simplex_commands
                            .send(SimplexCommand::LoadGroupMembers(chat_ref));
                    }
                }
                SimplexEvent::GroupRemoved(chat_ref) => self.remove_group(&chat_ref),
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
                SimplexEvent::ConnectionStarted => {
                    self.connection_loading = false;
                    self.connection_error = None;
                    self.notice = Some("Connection request sent; waiting for contact…".into());
                }
                SimplexEvent::ConnectionFailed(error) => {
                    self.connection_loading = false;
                    self.connection_error = Some(error);
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
                    self.connection_loading = false;
                    self.connection_error = None;
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

    pub(crate) fn selected_chat_is_writable(&self) -> bool {
        let Some(chat) = self.chats.get(self.selected_chat) else {
            return false;
        };
        !chat.chat_ref.0.starts_with('#')
            || !matches!(
                chat.group_status.as_deref(),
                Some("rejected" | "removed" | "left" | "deleted" | "invited" | "blocked")
            )
    }

    fn remove_group(&mut self, chat_ref: &ChatRef) {
        self.chats.retain(|chat| &chat.chat_ref != chat_ref);
        self.message_cache.remove(chat_ref);
        self.selected_chat = self.selected_chat.min(self.chats.len().saturating_sub(1));
        if self.loaded_chat.as_ref() == Some(chat_ref) {
            self.loaded_chat = None;
            self.messages.clear();
            self.composer.clear();
            self.composer_focused = false;
        }
        if self
            .group_management_dialog
            .as_ref()
            .map(|dialog| &dialog.chat_ref)
            == Some(chat_ref)
        {
            self.group_management_dialog = None;
        }
    }

    fn send_message(&mut self) {
        if self.composer.trim().is_empty() || self.sending || !self.selected_chat_is_writable() {
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

    fn show_connect_invitation(&mut self) {
        if self.active_user().is_none() {
            self.notice = Some("Create a profile first".into());
            return;
        }
        self.selected_chat = self.chats.len();
        self.input_mode = InputMode::ConnectInvitation;
        self.input.clear();
        self.connection_error = None;
    }

    fn connect_invitation(&mut self) {
        let Some(user_id) = self.active_user().map(|user| user.id) else {
            self.notice = Some("Create a profile first".into());
            return;
        };
        let link = self.input.trim().to_owned();
        if link.is_empty() || self.connection_loading {
            return;
        }
        self.input_mode = InputMode::None;
        self.input.clear();
        self.connection_loading = true;
        self.connection_error = None;
        if self
            .simplex_commands
            .send(SimplexCommand::ConnectInvitation { user_id, link })
            .is_err()
        {
            self.connection_loading = false;
            self.connection_error = Some("SimpleX worker is not available".into());
        }
    }

    fn handle_paste(&mut self, text: String) {
        if matches!(
            self.input_mode,
            InputMode::CreateProfile
                | InputMode::RenameProfile
                | InputMode::ConnectInvitation
                | InputMode::CreateGroup
                | InputMode::RenameGroup
        ) && !self.profile_create_pending
        {
            self.input.push_str(text.trim());
        }
    }

    fn create_group(&mut self) {
        let Some(user_id) = self.active_user().map(|user| user.id) else {
            self.notice = Some("Create a profile first".into());
            return;
        };
        let name = self.input.trim().to_owned();
        self.notice = Some("Creating group…".into());
        let _ = self
            .simplex_commands
            .send(SimplexCommand::CreateGroup { user_id, name });
    }

    fn open_group_management(&mut self) {
        let Some(chat_ref) = self.selected_chat_ref().cloned() else {
            return;
        };
        if !chat_ref.0.starts_with('#') {
            return;
        }
        self.group_management_dialog = Some(GroupManagementDialog {
            chat_ref: chat_ref.clone(),
            members: Vec::new(),
            selected: 0,
            adding: false,
            role_target: None,
            pending: true,
            error: None,
        });
        let _ = self
            .simplex_commands
            .send(SimplexCommand::LoadGroupMembers(chat_ref));
    }

    pub(crate) fn group_contacts(&self) -> Vec<(i64, String)> {
        let member_contacts: Vec<i64> = self
            .group_management_dialog
            .iter()
            .flat_map(|dialog| &dialog.members)
            .filter_map(|member| member.contact_id)
            .collect();
        self.chats
            .iter()
            .filter_map(|chat| {
                chat.chat_ref
                    .0
                    .strip_prefix('@')?
                    .parse()
                    .ok()
                    .filter(|id| !member_contacts.contains(id))
                    .map(|id| (id, chat.display_name.clone()))
            })
            .collect()
    }

    fn handle_group_management_key(&mut self, key: KeyEvent) {
        let contacts_len = self.group_contacts().len();
        let role_options = self
            .group_management_dialog
            .as_ref()
            .map(Self::available_group_roles)
            .unwrap_or_default();
        let Some(dialog) = &mut self.group_management_dialog else {
            return;
        };
        if dialog.pending {
            if key.code == KeyCode::Esc {
                self.group_management_dialog = None;
            }
            return;
        }
        let item_count = if dialog.role_target.is_some() {
            role_options.len()
        } else if dialog.adding {
            contacts_len
        } else {
            dialog.members.len()
        };
        let is_owner = dialog
            .members
            .iter()
            .any(|member| member.is_self && member.role == "owner");
        let can_admin = dialog
            .members
            .iter()
            .any(|member| member.is_self && matches!(member.role.as_str(), "admin" | "owner"));
        let is_current_member = dialog.members.iter().any(|member| {
            member.is_self
                && !matches!(
                    member.status.as_str(),
                    "rejected" | "removed" | "left" | "deleted"
                )
        });
        match key.code {
            KeyCode::Esc if dialog.adding => {
                dialog.adding = false;
                dialog.selected = 0;
            }
            KeyCode::Esc if dialog.role_target.is_some() => {
                dialog.role_target = None;
                dialog.selected = 0;
            }
            KeyCode::Esc => self.group_management_dialog = None,
            KeyCode::Up | KeyCode::Char('k') => dialog.selected = dialog.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                dialog.selected = (dialog.selected + 1).min(item_count.saturating_sub(1))
            }
            KeyCode::Char('a')
                if !dialog.adding
                    && dialog.role_target.is_none()
                    && is_current_member
                    && can_admin =>
            {
                dialog.adding = true;
                dialog.selected = 0;
            }
            KeyCode::Char('r')
                if !dialog.adding
                    && dialog.role_target.is_none()
                    && is_current_member
                    && is_owner =>
            {
                let chat_ref = dialog.chat_ref.clone();
                self.input = self.group_name(&chat_ref);
                self.input_mode = InputMode::RenameGroup;
            }
            KeyCode::Enter if dialog.adding => {
                let selected = dialog.selected;
                let chat_ref = dialog.chat_ref.clone();
                if let Some((contact_id, _)) = self.group_contacts().get(selected) {
                    let _ = self.simplex_commands.send(SimplexCommand::AddGroupMember {
                        chat_ref,
                        contact_id: *contact_id,
                    });
                    if let Some(dialog) = &mut self.group_management_dialog {
                        dialog.pending = true;
                    }
                }
            }
            KeyCode::Char('d')
                if !dialog.adding
                    && dialog.role_target.is_none()
                    && is_current_member
                    && can_admin =>
            {
                self.ask_remove_group_member()
            }
            KeyCode::Char('b')
                if !dialog.adding
                    && dialog.role_target.is_none()
                    && is_current_member
                    && Self::can_moderate_group(dialog) =>
            {
                self.ask_block_group_member()
            }
            KeyCode::Char('o')
                if !dialog.adding && dialog.role_target.is_none() && is_current_member =>
            {
                self.open_group_role_picker()
            }
            KeyCode::Enter if dialog.role_target.is_some() => self.change_group_member_role(),
            KeyCode::Char('l')
                if !dialog.adding && dialog.role_target.is_none() && is_current_member =>
            {
                self.ask_group_exit(false)
            }
            KeyCode::Char('x') if !dialog.adding && dialog.role_target.is_none() => {
                self.ask_group_delete()
            }
            _ => {}
        }
    }

    fn group_name(&self, chat_ref: &ChatRef) -> String {
        self.chats
            .iter()
            .find(|chat| &chat.chat_ref == chat_ref)
            .map_or_else(|| "Group".into(), |chat| chat.display_name.clone())
    }

    fn rename_group(&mut self) {
        let Some(user_id) = self.active_user().map(|user| user.id) else {
            return;
        };
        let Some(dialog) = &mut self.group_management_dialog else {
            self.input_mode = InputMode::None;
            return;
        };
        let name = self.input.trim().to_owned();
        dialog.pending = true;
        dialog.error = None;
        self.input_mode = InputMode::None;
        self.input.clear();
        let _ = self.simplex_commands.send(SimplexCommand::RenameGroup {
            user_id,
            chat_ref: dialog.chat_ref.clone(),
            name,
        });
    }

    fn ask_remove_group_member(&mut self) {
        let Some(dialog) = &self.group_management_dialog else {
            return;
        };
        let Some(member) = dialog.members.get(dialog.selected) else {
            return;
        };
        if member.is_self {
            self.notice = Some("Use l to leave the group".into());
            return;
        }
        self.group_confirmation = Some(GroupConfirmation {
            chat_ref: dialog.chat_ref.clone(),
            group_name: self.group_name(&dialog.chat_ref),
            action: GroupAction::Remove {
                member_id: member.id,
                name: member.display_name.clone(),
            },
        });
    }

    fn can_moderate_group(dialog: &GroupManagementDialog) -> bool {
        dialog.members.iter().any(|member| {
            member.is_self && matches!(member.role.as_str(), "moderator" | "admin" | "owner")
        })
    }

    fn role_rank(role: &str) -> usize {
        match role {
            "observer" => 0,
            "member" => 1,
            "moderator" => 2,
            "admin" => 3,
            "owner" => 4,
            _ => usize::MAX,
        }
    }

    pub(crate) fn available_group_roles(dialog: &GroupManagementDialog) -> Vec<&'static str> {
        let Some(own) = dialog.members.iter().find(|member| member.is_self) else {
            return Vec::new();
        };
        if Self::role_rank(&own.role) < Self::role_rank("admin") {
            return Vec::new();
        }
        ["observer", "member", "moderator", "admin", "owner"]
            .into_iter()
            .filter(|role| Self::role_rank(role) <= Self::role_rank(&own.role))
            .collect()
    }

    fn open_group_role_picker(&mut self) {
        let Some(dialog) = &mut self.group_management_dialog else {
            return;
        };
        let Some(target) = dialog.members.get(dialog.selected) else {
            return;
        };
        let own_role = dialog
            .members
            .iter()
            .find(|member| member.is_self)
            .map(|member| member.role.as_str())
            .unwrap_or("");
        let target_allowed = !target.is_self
            && Self::role_rank(own_role) >= Self::role_rank("admin")
            && Self::role_rank(own_role) >= Self::role_rank(&target.role)
            && !matches!(
                target.status.as_str(),
                "removed" | "left" | "pending_approval" | "pending_review"
            );
        if !target_allowed {
            self.notice = Some("You cannot change this member's role".into());
            return;
        }
        let current_role = target.role.clone();
        dialog.role_target = Some(dialog.selected);
        dialog.selected = Self::available_group_roles(dialog)
            .iter()
            .position(|role| *role == current_role)
            .unwrap_or(0);
    }

    fn change_group_member_role(&mut self) {
        let Some((chat_ref, member_id, member_name, role)) =
            self.group_management_dialog.as_ref().and_then(|dialog| {
                let target_index = dialog.role_target?;
                let role = Self::available_group_roles(dialog)
                    .get(dialog.selected)?
                    .to_string();
                let member = dialog.members.get(target_index)?;
                Some((
                    dialog.chat_ref.clone(),
                    member.id,
                    member.display_name.clone(),
                    role,
                ))
            })
        else {
            return;
        };
        self.group_confirmation = Some(GroupConfirmation {
            chat_ref: chat_ref.clone(),
            group_name: self.group_name(&chat_ref),
            action: GroupAction::ChangeRole {
                member_id,
                name: member_name,
                role,
            },
        });
    }

    fn ask_block_group_member(&mut self) {
        let Some(dialog) = &self.group_management_dialog else {
            return;
        };
        let Some(member) = dialog.members.get(dialog.selected) else {
            return;
        };
        if member.is_self {
            self.notice = Some("You cannot block yourself".into());
            return;
        }
        self.group_confirmation = Some(GroupConfirmation {
            chat_ref: dialog.chat_ref.clone(),
            group_name: self.group_name(&dialog.chat_ref),
            action: GroupAction::Block {
                member_id: member.id,
                name: member.display_name.clone(),
                blocked: !member.blocked,
            },
        });
    }

    fn ask_group_exit(&mut self, delete: bool) {
        let Some(dialog) = &self.group_management_dialog else {
            return;
        };
        self.group_confirmation = Some(GroupConfirmation {
            chat_ref: dialog.chat_ref.clone(),
            group_name: self.group_name(&dialog.chat_ref),
            action: if delete {
                GroupAction::Delete
            } else {
                GroupAction::Leave
            },
        });
    }

    fn ask_group_delete(&mut self) {
        let Some(dialog) = &self.group_management_dialog else {
            return;
        };
        let is_owner = dialog
            .members
            .iter()
            .any(|member| member.is_self && member.role == "owner");
        self.group_confirmation = Some(GroupConfirmation {
            chat_ref: dialog.chat_ref.clone(),
            group_name: self.group_name(&dialog.chat_ref),
            action: if is_owner {
                GroupAction::Delete
            } else {
                GroupAction::DeleteLocal
            },
        });
    }

    fn confirm_group_action(&mut self) {
        let Some(confirmation) = self.group_confirmation.take() else {
            return;
        };
        if let Some(dialog) = &mut self.group_management_dialog {
            dialog.pending = true;
        }
        match confirmation.action {
            GroupAction::Remove { member_id, .. } => {
                let _ = self
                    .simplex_commands
                    .send(SimplexCommand::RemoveGroupMember {
                        chat_ref: confirmation.chat_ref,
                        member_id,
                    });
            }
            GroupAction::Block {
                member_id, blocked, ..
            } => {
                let _ = self
                    .simplex_commands
                    .send(SimplexCommand::BlockGroupMember {
                        chat_ref: confirmation.chat_ref,
                        member_id,
                        blocked,
                    });
            }
            GroupAction::ChangeRole {
                member_id, role, ..
            } => {
                let _ = self
                    .simplex_commands
                    .send(SimplexCommand::ChangeGroupMemberRole {
                        chat_ref: confirmation.chat_ref,
                        member_id,
                        role,
                    });
            }
            GroupAction::Leave | GroupAction::Delete | GroupAction::DeleteLocal => {
                let Some(user_id) = self.active_user().map(|user| user.id) else {
                    return;
                };
                let command = if matches!(confirmation.action, GroupAction::Delete) {
                    SimplexCommand::DeleteGroup {
                        user_id,
                        chat_ref: confirmation.chat_ref,
                    }
                } else if matches!(confirmation.action, GroupAction::DeleteLocal) {
                    SimplexCommand::DeleteGroupLocally {
                        user_id,
                        chat_ref: confirmation.chat_ref,
                    }
                } else {
                    SimplexCommand::LeaveGroup {
                        user_id,
                        chat_ref: confirmation.chat_ref,
                    }
                };
                let _ = self.simplex_commands.send(command);
            }
        }
    }

    fn answer_group_invitation(&mut self, item_id: i64, group_id: i64, accept: bool) {
        let Some(user_id) = self.active_user().map(|user| user.id) else {
            return;
        };
        let Some(contact_chat_ref) = self.loaded_chat.clone() else {
            return;
        };
        self.notice = Some(
            if accept {
                "Joining group…"
            } else {
                "Declining invitation…"
            }
            .into(),
        );
        let _ = self
            .simplex_commands
            .send(SimplexCommand::AnswerGroupInvitation {
                user_id,
                contact_chat_ref,
                item_id,
                group_id,
                accept,
            });
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

    pub fn total_unread(&self) -> u64 {
        self.chats.iter().map(|chat| chat.unread_count).sum()
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

    fn rename_profile(&mut self) {
        let Some(profile) = self.profiles.get(self.selected_profile) else {
            self.input_mode = InputMode::None;
            self.input.clear();
            return;
        };
        let display_name = self.input.trim().to_owned();
        if display_name.is_empty() {
            return;
        }
        let user_id = profile.id;
        self.input_mode = InputMode::None;
        self.input.clear();
        self.notice = Some(format!("Renaming profile {}…", profile.display_name));
        let _ = self.simplex_commands.send(SimplexCommand::RenameProfile {
            user_id,
            display_name,
        });
    }

    fn open_chat_deletion(&mut self) {
        let Some(chat_ref) = self.selected_chat_ref().cloned() else {
            return;
        };
        self.chat_deletion_dialog = Some(ChatDeletionDialog {
            chat_ref: chat_ref.clone(),
            settings: None,
            features: None,
            selected: 0,
            pending: true,
            error: None,
        });
        let _ = self
            .simplex_commands
            .send(SimplexCommand::LoadChatDeletion {
                chat_ref: chat_ref.clone(),
            });
        let _ = self
            .simplex_commands
            .send(SimplexCommand::LoadConversationFeatures { chat_ref });
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

    fn select_chat_setting_up(&mut self) {
        if let Some(dialog) = &mut self.chat_deletion_dialog {
            dialog.selected = dialog.selected.saturating_sub(1);
        }
    }

    fn select_chat_setting_down(&mut self) {
        if let Some(dialog) = &mut self.chat_deletion_dialog {
            dialog.selected = dialog.selected.saturating_add(1).min(8);
        }
    }

    fn activate_chat_setting(&mut self) {
        let Some(dialog) = &self.chat_deletion_dialog else {
            return;
        };
        if dialog.pending {
            return;
        }
        match dialog.selected {
            0 => self.cycle_chat_deletion(),
            1 => self.toggle_conversation_feature(ChatFeature::FullDeletion),
            2 => self.toggle_conversation_feature(ChatFeature::Reactions),
            3 => self.toggle_conversation_feature(ChatFeature::VoiceMessages),
            4 => self.toggle_conversation_feature(ChatFeature::FilesAndMedia),
            5 => self.ask_chat_delete(ChatDeleteMode::Conversation),
            6 => self.ask_chat_delete(ChatDeleteMode::Contact),
            7 => self.ask_chat_delete(ChatDeleteMode::BlockContact),
            8 => self.chat_deletion_dialog = None,
            _ => {}
        }
    }

    fn toggle_conversation_feature(&mut self, feature: ChatFeature) {
        let Some(dialog) = &mut self.chat_deletion_dialog else {
            return;
        };
        let Some(features) = &dialog.features else {
            return;
        };
        let enabled = match feature {
            ChatFeature::FullDeletion => !features.full_deletion,
            ChatFeature::Reactions => !features.reactions,
            ChatFeature::VoiceMessages => !features.voice_messages,
            ChatFeature::FilesAndMedia => !features.files_and_media,
        };
        dialog.pending = true;
        dialog.error = None;
        let _ = self
            .simplex_commands
            .send(SimplexCommand::SetConversationFeature {
                chat_ref: dialog.chat_ref.clone(),
                feature,
                enabled,
            });
    }

    fn ask_chat_delete(&mut self, mode: ChatDeleteMode) {
        let Some(dialog) = &self.chat_deletion_dialog else {
            return;
        };
        if mode != ChatDeleteMode::Conversation && !dialog.chat_ref.0.starts_with('@') {
            self.notice = Some("Contact actions are only available in direct chats".into());
            return;
        }
        let chat_name = self
            .chats
            .iter()
            .find(|chat| chat.chat_ref == dialog.chat_ref)
            .map_or_else(|| "Chat".into(), |chat| chat.display_name.clone());
        self.chat_delete_confirmation = Some(ChatDeleteConfirmation {
            chat_ref: dialog.chat_ref.clone(),
            chat_name,
            mode,
        });
    }

    fn confirm_chat_delete(&mut self) {
        let Some(user_id) = self.active_user().map(|user| user.id) else {
            return;
        };
        let Some(confirmation) = self.chat_delete_confirmation.take() else {
            return;
        };
        if let Some(dialog) = &mut self.chat_deletion_dialog {
            dialog.pending = true;
            dialog.error = None;
        }
        let _ = self.simplex_commands.send(SimplexCommand::DeleteChat {
            user_id,
            chat_ref: confirmation.chat_ref,
            mode: confirmation.mode,
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
        let result = std::fs::create_dir_all(&self.data_directory)
            .and_then(|()| self.preferences.save(&self.data_directory));
        match result {
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
        message.reactions.push(libsimplex_rs::MessageReaction {
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
                    group_status: None,
                },
                ChatSummary {
                    chat_ref: ChatRef("@2".into()),
                    display_name: "bob".into(),
                    unread_count: 0,
                    group_status: None,
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
                group_status: None,
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
    async fn pasted_invitation_is_sent_as_a_typed_wrapper_command() {
        let (commands, receiver) = mpsc::channel();
        let mut app = App {
            startup: StartupState::Ready(User {
                id: 7,
                display_name: "alice".into(),
                notifications: true,
                active: true,
            }),
            simplex_commands: commands,
            ..App::default()
        };

        app.handle_key_events(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.input_mode, InputMode::ConnectInvitation);
        app.handle_paste("  https://simplex.chat/contact#example\n".into());
        app.handle_key_events(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();

        let SimplexCommand::ConnectInvitation { user_id, link } = receiver.try_recv().unwrap()
        else {
            panic!("expected typed connect-invitation command")
        };
        assert_eq!(user_id, 7);
        assert_eq!(link, "https://simplex.chat/contact#example");
        assert!(app.connection_loading);
        assert_eq!(app.input_mode, InputMode::None);
    }

    #[tokio::test]
    async fn clicking_send_button_sends_the_draft() {
        let (commands, receiver) = mpsc::channel();
        let mut app = App {
            chats: vec![ChatSummary {
                chat_ref: ChatRef("#3".into()),
                display_name: "team".into(),
                unread_count: 0,
                group_status: None,
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
                group_status: None,
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
                    sender_name: None,
                    reactions: Vec::new(),
                    attachment: None,
                    group_invitation: None,
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
                group_status: None,
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
                    sender_name: None,
                    reactions: Vec::new(),
                    attachment: None,
                    group_invitation: None,
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
                group_status: None,
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
                    sender_name: None,
                    reactions: Vec::new(),
                    attachment: None,
                    group_invitation: None,
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
                group_status: None,
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
                    group_status: None,
                },
                ChatSummary {
                    chat_ref: ChatRef("@2".into()),
                    display_name: "bob".into(),
                    unread_count: 0,
                    group_status: None,
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
            sender_name: None,
            reactions: Vec::new(),
            attachment: None,
            group_invitation: None,
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
    async fn profile_rename_uses_a_typed_wrapper_command() {
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

        app.handle_key_events(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.input_mode, InputMode::RenameProfile);
        assert_eq!(app.input, "work");
        app.input = "personal".into();
        app.handle_key_events(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();

        let SimplexCommand::RenameProfile {
            user_id,
            display_name,
        } = receiver.try_recv().unwrap()
        else {
            panic!("expected rename-profile command")
        };
        assert_eq!(user_id, 9);
        assert_eq!(display_name, "personal");
    }

    #[tokio::test]
    async fn empty_profile_row_accepts_a_name_and_creates_profile() {
        let (commands, receiver) = mpsc::channel();
        let mut app = App {
            section: Section::Profiles,
            simplex_commands: commands,
            ..App::default()
        };

        // The row looks like an editor, so typing must focus it immediately;
        // no preliminary Enter should be necessary.
        for character in "Alice (Private)".chars() {
            app.handle_key_events(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
                .unwrap();
        }
        assert_eq!(app.input_mode, InputMode::CreateProfile);
        assert_eq!(app.input, "Alice (Private)");
        app.handle_key_events(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();

        let SimplexCommand::CreateProfile(display_name) = receiver.try_recv().unwrap() else {
            panic!("expected create-profile command")
        };
        assert_eq!(display_name, "Alice (Private)");
    }

    #[tokio::test]
    async fn clicking_empty_profile_row_focuses_the_name_editor() {
        let mut app = App {
            section: Section::Profiles,
            ..App::default()
        };

        app.handle_app_event(AppEvent::SelectIndex(0));

        assert_eq!(app.input_mode, InputMode::CreateProfile);
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
                group_status: None,
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
                SimplexCommand::LoadChat(_) | SimplexCommand::LoadConversationFeatures { .. } => {}
                command => panic!("unexpected command: {command:?}"),
            }
        };
        assert_eq!((user_id, changed, seconds), (3, chat_ref, Some(0)));
        assert!(app.chat_deletion_dialog.as_ref().unwrap().pending);
    }

    #[tokio::test]
    async fn destructive_chat_actions_require_explicit_confirmation() {
        let (commands, receiver) = mpsc::channel();
        let confirmation = ChatDeleteConfirmation {
            chat_ref: ChatRef("@7".into()),
            chat_name: "bob".into(),
            mode: ChatDeleteMode::BlockContact,
        };
        let mut app = App {
            startup: StartupState::Ready(User {
                id: 3,
                display_name: "alice".into(),
                notifications: true,
                active: true,
            }),
            chat_delete_confirmation: Some(confirmation.clone()),
            simplex_commands: commands,
            ..App::default()
        };

        app.handle_key_events(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert!(app.chat_delete_confirmation.is_none());
        assert!(receiver.try_recv().is_err());

        app.chat_delete_confirmation = Some(confirmation);
        app.handle_key_events(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE))
            .unwrap();
        let SimplexCommand::DeleteChat {
            user_id,
            chat_ref,
            mode,
        } = receiver.try_recv().unwrap()
        else {
            panic!("expected typed chat-delete command")
        };
        assert_eq!(user_id, 3);
        assert_eq!(chat_ref, ChatRef("@7".into()));
        assert_eq!(mode, ChatDeleteMode::BlockContact);
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
                sender_name: None,
                reactions: Vec::new(),
                attachment: Some(libsimplex_rs::Attachment {
                    id: 41,
                    name: "archive.zip".into(),
                    size: 100,
                    kind: libsimplex_rs::AttachmentKind::File,
                    status: "rcvTransfer".into(),
                    progress: Some(50),
                    path: None,
                }),
                group_invitation: None,
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

    #[tokio::test]
    async fn total_unread_sums_all_conversations() {
        let app = App {
            chats: vec![
                ChatSummary {
                    chat_ref: ChatRef("@1".into()),
                    display_name: "alice".into(),
                    unread_count: 2,
                    group_status: None,
                },
                ChatSummary {
                    chat_ref: ChatRef("@2".into()),
                    display_name: "bob".into(),
                    unread_count: 3,
                    group_status: None,
                },
            ],
            ..App::default()
        };
        assert_eq!(app.total_unread(), 5);
    }

    #[tokio::test]
    async fn owner_deleted_group_is_removed_from_the_active_conversation() {
        let (event_sender, simplex_events) = mpsc::channel();
        let mut app = App {
            chats: vec![ChatSummary {
                chat_ref: ChatRef("#7".into()),
                display_name: "Friends".into(),
                unread_count: 0,
                group_status: Some("connected".into()),
            }],
            loaded_chat: Some(ChatRef("#7".into())),
            composer: "draft".into(),
            composer_focused: true,
            messages: vec![Message {
                id: 1,
                text: "hello".into(),
                timestamp: String::new(),
                outgoing: false,
                sender_name: None,
                reactions: Vec::new(),
                attachment: None,
                group_invitation: None,
            }],
            simplex_events,
            ..App::default()
        };
        event_sender
            .send(SimplexEvent::GroupRemoved(ChatRef("#7".into())))
            .unwrap();

        app.tick();

        assert!(app.chats.is_empty());
        assert_eq!(app.loaded_chat, None);
        assert!(app.messages.is_empty());
        assert!(app.composer.is_empty());
        assert!(!app.composer_focused);
    }

    #[tokio::test]
    async fn former_group_member_cannot_focus_or_send() {
        let (simplex_commands, receiver) = mpsc::channel();
        let mut app = App {
            chats: vec![ChatSummary {
                chat_ref: ChatRef("#7".into()),
                display_name: "Friends".into(),
                unread_count: 0,
                group_status: Some("left".into()),
            }],
            composer: "draft".into(),
            simplex_commands,
            ..App::default()
        };

        app.handle_key_events(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE))
            .unwrap();
        app.send_message();

        assert!(!app.composer_focused);
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn group_creation_uses_a_typed_wrapper_command() {
        let (commands, receiver) = mpsc::channel();
        let mut app = App {
            section: Section::Chats,
            startup: StartupState::Ready(User {
                id: 3,
                display_name: "Alice".into(),
                notifications: true,
                active: true,
            }),
            simplex_commands: commands,
            ..App::default()
        };

        app.handle_key_events(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE))
            .unwrap();
        for character in "Friends (Vienna)".chars() {
            app.handle_key_events(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
                .unwrap();
        }
        app.handle_key_events(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();

        let SimplexCommand::CreateGroup { user_id, name } = receiver.try_recv().unwrap() else {
            panic!("expected create-group command")
        };
        assert_eq!(user_id, 3);
        assert_eq!(name, "Friends (Vienna)");
    }

    #[tokio::test]
    async fn group_management_loads_members_and_confirms_removal() {
        let (commands, receiver) = mpsc::channel();
        let (event_sender, simplex_events) = mpsc::channel();
        let mut app = App {
            section: Section::Chats,
            chats: vec![ChatSummary {
                chat_ref: ChatRef("#7".into()),
                display_name: "Friends".into(),
                unread_count: 0,
                group_status: None,
            }],
            simplex_commands: commands,
            simplex_events,
            ..App::default()
        };

        app.handle_key_events(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(
            receiver.try_recv().unwrap(),
            SimplexCommand::LoadGroupMembers(ChatRef(ref value)) if value == "#7"
        ));
        event_sender
            .send(SimplexEvent::GroupMembersLoaded {
                chat_ref: ChatRef("#7".into()),
                members: vec![
                    GroupMember {
                        id: 12,
                        contact_id: Some(22),
                        display_name: "Bob".into(),
                        role: "member".into(),
                        status: "connected".into(),
                        is_self: false,
                        blocked: false,
                    },
                    GroupMember {
                        id: 13,
                        contact_id: None,
                        display_name: "Alice".into(),
                        role: "owner".into(),
                        status: "creator".into(),
                        is_self: true,
                        blocked: false,
                    },
                ],
            })
            .unwrap();
        app.tick();
        assert!(matches!(
            receiver.try_recv().unwrap(),
            SimplexCommand::LoadChat(ChatRef(ref value)) if value == "#7"
        ));
        app.handle_key_events(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE))
            .unwrap();
        app.handle_key_events(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE))
            .unwrap();

        assert!(matches!(
            receiver.try_recv().unwrap(),
            SimplexCommand::RemoveGroupMember {
                chat_ref: ChatRef(ref value),
                member_id: 12
            } if value == "#7"
        ));
    }

    #[tokio::test]
    async fn group_management_renames_via_typed_wrapper_command() {
        let (commands, receiver) = mpsc::channel();
        let mut app = App {
            section: Section::Chats,
            startup: StartupState::Ready(User {
                id: 3,
                display_name: "Alice".into(),
                notifications: true,
                active: true,
            }),
            chats: vec![ChatSummary {
                chat_ref: ChatRef("#7".into()),
                display_name: "Friends".into(),
                unread_count: 0,
                group_status: None,
            }],
            group_management_dialog: Some(GroupManagementDialog {
                chat_ref: ChatRef("#7".into()),
                members: vec![GroupMember {
                    id: 3,
                    contact_id: None,
                    display_name: "Alice".into(),
                    role: "owner".into(),
                    status: "creator".into(),
                    is_self: true,
                    blocked: false,
                }],
                selected: 0,
                adding: false,
                role_target: None,
                pending: false,
                error: None,
            }),
            simplex_commands: commands,
            ..App::default()
        };

        app.handle_key_events(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE))
            .unwrap();
        assert_eq!(app.input_mode, InputMode::RenameGroup);
        assert_eq!(app.input, "Friends");
        app.input = "Friends (Vienna)".into();
        app.handle_key_events(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();

        assert!(matches!(
            receiver.try_recv().unwrap(),
            SimplexCommand::RenameGroup {
                user_id: 3,
                chat_ref: ChatRef(ref value),
                ref name,
            } if value == "#7" && name == "Friends (Vienna)"
        ));
    }

    #[tokio::test]
    async fn moderator_can_block_a_group_member_for_all() {
        let (commands, receiver) = mpsc::channel();
        let mut app = App {
            chats: vec![ChatSummary {
                chat_ref: ChatRef("#7".into()),
                display_name: "Friends".into(),
                unread_count: 0,
                group_status: Some("connected".into()),
            }],
            group_management_dialog: Some(GroupManagementDialog {
                chat_ref: ChatRef("#7".into()),
                members: vec![
                    GroupMember {
                        id: 12,
                        contact_id: Some(22),
                        display_name: "Bob".into(),
                        role: "member".into(),
                        status: "connected".into(),
                        is_self: false,
                        blocked: false,
                    },
                    GroupMember {
                        id: 13,
                        contact_id: None,
                        display_name: "Alice".into(),
                        role: "moderator".into(),
                        status: "connected".into(),
                        is_self: true,
                        blocked: false,
                    },
                ],
                selected: 0,
                adding: false,
                role_target: None,
                pending: false,
                error: None,
            }),
            simplex_commands: commands,
            ..App::default()
        };

        app.handle_key_events(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE))
            .unwrap();
        app.handle_key_events(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE))
            .unwrap();

        assert!(matches!(
            receiver.try_recv().unwrap(),
            SimplexCommand::BlockGroupMember {
                chat_ref: ChatRef(ref value),
                member_id: 12,
                blocked: true,
            } if value == "#7"
        ));
    }

    #[tokio::test]
    async fn admin_can_assign_roles_up_to_admin_but_not_owner() {
        let (commands, receiver) = mpsc::channel();
        let mut app = App {
            group_management_dialog: Some(GroupManagementDialog {
                chat_ref: ChatRef("#7".into()),
                members: vec![
                    GroupMember {
                        id: 12,
                        contact_id: Some(22),
                        display_name: "Bob".into(),
                        role: "member".into(),
                        status: "connected".into(),
                        is_self: false,
                        blocked: false,
                    },
                    GroupMember {
                        id: 13,
                        contact_id: None,
                        display_name: "Alice".into(),
                        role: "admin".into(),
                        status: "connected".into(),
                        is_self: true,
                        blocked: false,
                    },
                ],
                selected: 0,
                adding: false,
                role_target: None,
                pending: false,
                error: None,
            }),
            simplex_commands: commands,
            ..App::default()
        };

        assert_eq!(
            App::available_group_roles(app.group_management_dialog.as_ref().unwrap()),
            vec!["observer", "member", "moderator", "admin"]
        );
        app.handle_key_events(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE))
            .unwrap();
        app.handle_key_events(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
            .unwrap();
        app.handle_key_events(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        app.handle_key_events(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE))
            .unwrap();

        assert!(matches!(
            receiver.try_recv().unwrap(),
            SimplexCommand::ChangeGroupMemberRole {
                chat_ref: ChatRef(ref value),
                member_id: 12,
                ref role,
            } if value == "#7" && role == "moderator"
        ));
    }
}
