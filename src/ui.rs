use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, Padding, Paragraph, Tabs, Widget, Wrap},
};

use crate::app::{App, InputMode, Section, StartupState};
use crate::preferences::Theme;

impl Widget for &App {
    fn render(self, area: Rect, buf: &mut Buffer) {
        match self.preferences.theme {
            Theme::Terminal => {}
            Theme::Dark => buf.set_style(
                area,
                Style::default()
                    .fg(Color::Rgb(225, 230, 238))
                    .bg(Color::Rgb(15, 18, 24)),
            ),
            Theme::Light => buf.set_style(
                area,
                Style::default()
                    .fg(Color::Rgb(30, 35, 42))
                    .bg(Color::Rgb(232, 235, 239)),
            ),
        }
        self.area.set(area);
        self.composer_area.set(Rect::default());
        self.send_area.set(Rect::default());
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(32), Constraint::Percentage(68)])
            .split(area);
        let sidebar = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(columns[0]);

        render_tabs(self, sidebar[0], buf);
        render_sidebar(self, sidebar[1], buf);
        render_detail(self, columns[1], buf);
    }
}

fn panel(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(format!(" {title} "))
}

fn render_tabs(app: &App, area: Rect, buf: &mut Buffer) {
    Tabs::new([" Chats ", " Profiles ", " Settings "])
        .select(match app.section {
            Section::Chats => 0,
            Section::Profiles => 1,
            Section::Settings => 2,
        })
        .divider("│")
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" SimpleX "),
        )
        .render(area, buf);
}

fn render_sidebar(app: &App, area: Rect, buf: &mut Buffer) {
    let (title, entries, selected): (_, Vec<String>, _) = match app.section {
        Section::Chats => (
            "Conversations",
            app.chats
                .iter()
                .map(|chat| {
                    if chat.unread_count == 0 {
                        chat.display_name.clone()
                    } else {
                        format!("{} ({})", chat.display_name, chat.unread_count)
                    }
                })
                .collect(),
            app.selected_chat,
        ),
        Section::Profiles => (
            "Profiles",
            app.profiles
                .iter()
                .map(|profile| {
                    format!(
                        "{}{}",
                        profile.display_name,
                        if profile.active { "  [active]" } else { "" }
                    )
                })
                .chain(std::iter::once("＋ Create profile".into()))
                .collect(),
            app.selected_profile,
        ),
        Section::Settings => (
            "Settings",
            App::SETTINGS.iter().map(|s| (*s).into()).collect(),
            app.selected_setting,
        ),
    };
    let items = entries.iter().enumerate().map(|(index, name)| {
        let marker = if index == selected { "● " } else { "  " };
        ListItem::new(Line::from(vec![Span::styled(
            format!("{marker}{name}"),
            if index == selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            },
        )]))
    });

    List::new(items)
        .block(panel(title).padding(Padding::vertical(1)))
        .render(area, buf);
}

fn render_detail(app: &App, area: Rect, buf: &mut Buffer) {
    match app.section {
        Section::Chats => render_chat(app, area, buf),
        Section::Profiles => render_profile(app, area, buf),
        Section::Settings => render_settings(app, area, buf),
    }
}

fn render_profile(app: &App, area: Rect, buf: &mut Buffer) {
    if app.input_mode == InputMode::CreateProfile || app.selected_profile >= app.profiles.len() {
        let mut input = app.input.clone();
        if app.input_mode == InputMode::CreateProfile {
            input.push('▏');
        }
        let hint = if app.input_mode == InputMode::CreateProfile {
            "Enter: create · Esc: cancel"
        } else {
            "Press Enter or n to start"
        };
        Paragraph::new(format!(
            "Create a new SimpleX profile\n\nDisplay name\n{input}\n\n{hint}"
        ))
        .block(panel("New profile").padding(Padding::new(2, 2, 1, 1)))
        .wrap(Wrap { trim: false })
        .render(area, buf);
        return;
    }
    let profile = &app.profiles[app.selected_profile];
    Paragraph::new(format!(
        "Display name\n{}\n\nNotifications\n{}\n\nStatus\n{}\n\nEnter: activate · n: new profile",
        profile.display_name,
        enabled(profile.notifications),
        if profile.active { "Active" } else { "Inactive" },
    ))
    .block(panel(&profile.display_name).padding(Padding::new(2, 2, 1, 1)))
    .wrap(Wrap { trim: false })
    .render(area, buf);
}

fn render_chat(app: &App, area: Rect, buf: &mut Buffer) {
    let Some(summary) = app.chats.get(app.selected_chat) else {
        let (title, text) = match &app.startup {
            StartupState::Loading => ("SimpleX", "Opening the local SimpleX database…"),
            StartupState::NoActiveUser => (
                "Welcome",
                "No SimpleX profile exists yet.\n\nProfile creation is the next onboarding step.",
            ),
            StartupState::Ready(user) => (
                "Conversations",
                if user.display_name.is_empty() {
                    "No conversations yet."
                } else {
                    "No conversations yet. Create or connect to a contact to begin."
                },
            ),
            StartupState::Failed(error) => ("SimpleX error", error.as_str()),
        };
        Paragraph::new(text)
            .block(panel(title).padding(Padding::new(2, 2, 1, 1)))
            .wrap(Wrap { trim: false })
            .render(area, buf);
        return;
    };
    let chat = summary.display_name.as_str();
    let inner = panel(chat).padding(Padding::new(2, 2, 1, 1)).inner(area);
    panel(chat)
        .padding(Padding::new(2, 2, 1, 1))
        .render(area, buf);

    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(5),
        Constraint::Length(1),
    ])
    .split(inner);

    Paragraph::new("Today")
        .alignment(Alignment::Center)
        .fg(Color::DarkGray)
        .render(rows[0], buf);
    let available = usize::from(rows[1].height);
    let start = app.messages.len().saturating_sub(available);
    let lines: Vec<Line> = app.messages[start..]
        .iter()
        .flat_map(|message| {
            let time = message.timestamp.get(11..16).unwrap_or("");
            let prefix = if message.outgoing { "You" } else { chat };
            let color = if message.outgoing {
                Color::Rgb(40, 210, 130)
            } else {
                Color::Rgb(70, 160, 255)
            };
            let message_line = Line::from(vec![
                Span::styled(
                    format!("{time} {prefix}: "),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::raw(&message.text),
            ]);
            if app.preferences.compact_messages {
                vec![message_line]
            } else {
                vec![message_line, Line::default()]
            }
        })
        .collect();
    let content = if let Some(error) = &app.chat_error {
        Paragraph::new(error.as_str()).fg(Color::Red)
    } else if app.chat_loading || app.loaded_chat.as_ref() != Some(&summary.chat_ref) {
        Paragraph::new("Loading messages…").fg(Color::DarkGray)
    } else if lines.is_empty() {
        Paragraph::new("No messages in this conversation.").fg(Color::DarkGray)
    } else {
        Paragraph::new(lines)
    };
    content.wrap(Wrap { trim: false }).render(rows[1], buf);

    let composer_columns =
        Layout::horizontal([Constraint::Min(10), Constraint::Length(10)]).split(rows[2]);
    app.composer_area.set(composer_columns[0]);
    app.send_area.set(composer_columns[1]);
    let mut draft = app.composer.clone();
    if app.composer_focused {
        draft.push('▏');
    }
    Paragraph::new(draft)
        .block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .title(" Message ")
                .border_style(Style::default().fg(if app.composer_focused {
                    Color::Cyan
                } else {
                    Color::DarkGray
                })),
        )
        .wrap(Wrap { trim: false })
        .render(composer_columns[0], buf);
    let button_text = if app.sending { "Sending…" } else { "Send" };
    Paragraph::new(button_text)
        .alignment(Alignment::Center)
        .block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .title(" ↵ ")
                .border_style(Style::default().fg(if app.sending {
                    Color::DarkGray
                } else {
                    Color::Rgb(40, 210, 130)
                })),
        )
        .render(composer_columns[1], buf);

    if let Some(footer) = rows.last() {
        Paragraph::new(
            "Enter: send · Shift+Enter: newline · Alt+Enter: fallback · Esc: leave composer",
        )
        .alignment(Alignment::Center)
        .fg(Color::DarkGray)
        .render(*footer, buf);
    }
}

fn render_settings(app: &App, area: Rect, buf: &mut Buffer) {
    let title = App::SETTINGS[app.selected_setting];
    let body = match app.selected_setting {
        0 => format!(
            "Active profile\n{}\n\nProfiles\n{}\n\nData directory\n~/.simplex-tui",
            app.active_user().map_or("None", |user| user.display_name.as_str()),
            app.profiles.len()
        ),
        1 => format!(
            "Profile notifications\n{}\n\nMessage preview\n{}\n\nEnter/Space: toggle notifications · p: toggle preview",
            app.active_user().map_or("Unavailable", |user| enabled(user.notifications)),
            enabled(app.preferences.message_preview)
        ),
        2 => format!(
            "Local database\nManaged by SimpleX\n\nAutomatic message deletion\n{}\n\nEnter/Space: cycle Off → 1 day → 7 days → 30 days",
            auto_delete_label(app.auto_delete_seconds)
        ),
        3 => format!(
            "Theme\n{}\n\nCompact messages\n{}\n\nEnter/Space: change theme · c: toggle compact mode",
            app.preferences.theme.label(),
            enabled(app.preferences.compact_messages)
        ),
        _ => "simplex-tui\nA private terminal client powered by the official SimpleX Chat library.\n\nSimpleX data and application state are stored under ~/.simplex-tui.".into(),
    };
    Paragraph::new(body)
        .block(panel(title).padding(Padding::new(2, 2, 1, 1)))
        .wrap(Wrap { trim: false })
        .render(area, buf);

    if let Some(notice) = &app.notice {
        let notice_area = Rect::new(
            area.x.saturating_add(3),
            area.bottom().saturating_sub(3),
            area.width.saturating_sub(6),
            1,
        );
        Paragraph::new(notice.as_str())
            .alignment(Alignment::Center)
            .fg(Color::Cyan)
            .render(notice_area, buf);
    }
}

fn enabled(value: bool) -> &'static str {
    if value { "Enabled" } else { "Disabled" }
}

fn auto_delete_label(seconds: i64) -> &'static str {
    match seconds {
        86_400 => "After 1 day",
        604_800 => "After 7 days",
        2_592_000 => "After 30 days",
        _ => "Off",
    }
}
