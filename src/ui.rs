use qrcode::QrCode;
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, Padding, Paragraph, Tabs, Widget, Wrap,
    },
};
use tui_qrcode::{Colors, QrCodeWidget, Scaling};

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
        self.jump_to_latest_area.set(Rect::default());
        self.delete_cancel_area.set(Rect::default());
        self.delete_ok_area.set(Rect::default());
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
        if self.input_mode == InputMode::ConfirmDeleteProfile {
            render_delete_profile_confirmation(self, area, buf);
        }
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
                .chain(app.active_user().map(|_| "＋ Invite contact".to_owned()))
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
        let unread = app.section == Section::Chats
            && app
                .chats
                .get(index)
                .is_some_and(|chat| chat.unread_count > 0);
        let style = if unread {
            Style::default()
                .fg(Color::LightBlue)
                .add_modifier(Modifier::BOLD)
        } else if index == selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        ListItem::new(Line::from(vec![Span::styled(
            format!("{marker}{name}"),
            style,
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
        "Display name\n{}\n\nStatus\n{}\n\nEnter: activate · n: new profile · d: delete",
        profile.display_name,
        if profile.active { "Active" } else { "Inactive" },
    ))
    .block(panel(&profile.display_name).padding(Padding::new(2, 2, 1, 1)))
    .wrap(Wrap { trim: false })
    .render(area, buf);
}

fn render_delete_profile_confirmation(app: &App, area: Rect, buf: &mut Buffer) {
    let Some(profile) = app.profiles.get(app.selected_profile) else {
        return;
    };
    let width = area.width.min(68);
    let height = area.height.min(9);
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    Clear.render(popup, buf);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Red))
        .title(" Delete profile ");
    let inner = block.inner(popup);
    block.render(popup, buf);
    let rows = Layout::vertical([Constraint::Min(2), Constraint::Length(3)]).split(inner);
    Paragraph::new(format!(
        "Are you sure that you want to delete profile {}?",
        profile.display_name
    ))
    .alignment(Alignment::Center)
    .wrap(Wrap { trim: false })
    .render(rows[0], buf);
    let buttons =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[1]);
    app.delete_cancel_area.set(buttons[0]);
    app.delete_ok_area.set(buttons[1]);
    Paragraph::new("Cancel (Enter)")
        .alignment(Alignment::Center)
        .block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .render(buttons[0], buf);
    Paragraph::new("OK (y)")
        .alignment(Alignment::Center)
        .block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Red)),
        )
        .render(buttons[1], buf);
}

fn render_chat(app: &App, area: Rect, buf: &mut Buffer) {
    if app.active_user().is_some() && app.selected_chat == app.chats.len() {
        render_invitation(app, area, buf);
        return;
    }
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
                    "No conversations yet.\n\nPress n to create a one-time invitation link."
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
        Constraint::Length(1),
        Constraint::Length(5),
        Constraint::Length(1),
    ])
    .split(inner);

    Paragraph::new("Today")
        .alignment(Alignment::Center)
        .fg(Color::DarkGray)
        .render(rows[0], buf);
    let available = usize::from(rows[1].height);
    let bubble_specs: Vec<(u16, u16)> = app
        .messages
        .iter()
        .map(|message| {
            let time = message.timestamp.get(11..16).unwrap_or("");
            let prefix = if message.outgoing { "You" } else { chat };
            bubble_size(&message.text, prefix, time, rows[1].width)
        })
        .collect();
    let gap = usize::from(!app.preferences.compact_messages);
    let mut top_page_messages: usize = 0;
    let mut top_page_height: usize = 0;
    for &(height, _) in &bubble_specs {
        let required = usize::from(height) + usize::from(top_page_messages > 0) * gap;
        if top_page_messages > 0 && top_page_height.saturating_add(required) > available {
            break;
        }
        top_page_messages += 1;
        top_page_height = top_page_height.saturating_add(required);
        if top_page_height >= available {
            break;
        }
    }
    let max_scroll = app.messages.len().saturating_sub(top_page_messages);
    app.max_chat_scroll.set(max_scroll);
    let effective_scroll = app.chat_scroll.min(max_scroll);
    let visible_end = app.messages.len().saturating_sub(effective_scroll);
    let mut visible_start = visible_end;
    let mut visible_height = 0usize;
    while visible_start > 0 {
        let height = usize::from(bubble_specs[visible_start - 1].0);
        let required = height + usize::from(visible_start < visible_end) * gap;
        if visible_start < visible_end && visible_height.saturating_add(required) > available {
            break;
        }
        visible_start -= 1;
        visible_height = visible_height.saturating_add(required);
        if visible_height >= available {
            break;
        }
    }
    if let Some(error) = &app.chat_error {
        Paragraph::new(error.as_str())
            .fg(Color::Red)
            .render(rows[1], buf);
    } else if app.chat_loading || app.loaded_chat.as_ref() != Some(&summary.chat_ref) {
        Paragraph::new("Loading messages…")
            .fg(Color::DarkGray)
            .render(rows[1], buf);
    } else if visible_start == visible_end {
        Paragraph::new("No messages in this conversation.")
            .fg(Color::DarkGray)
            .render(rows[1], buf);
    } else {
        render_message_bubbles(
            app,
            chat,
            rows[1],
            buf,
            &bubble_specs,
            visible_start,
            visible_end,
            gap,
            visible_height,
        );
    }

    let total_height = bubble_specs
        .iter()
        .map(|(height, _)| usize::from(*height))
        .sum::<usize>()
        + bubble_specs.len().saturating_sub(1) * gap;
    let top_line = bubble_specs[..visible_start]
        .iter()
        .map(|(height, _)| usize::from(*height))
        .sum::<usize>()
        + visible_start.saturating_sub(1) * gap;
    render_chat_scrollbar(rows[1], buf, total_height, available, top_line);

    if app.chat_scroll > 0 && rows[1].width > 4 && rows[1].height > 0 {
        let label = if app.new_messages_below > 0 {
            format!(" ↓ {} new ", app.new_messages_below)
        } else {
            " ↓ latest ".into()
        };
        let width = u16::try_from(label.chars().count())
            .unwrap_or(rows[1].width)
            .min(rows[1].width);
        let jump_area = Rect::new(
            rows[1].right().saturating_sub(width),
            rows[1].bottom().saturating_sub(1),
            width,
            1,
        );
        app.jump_to_latest_area.set(jump_area);
        Paragraph::new(label)
            .alignment(Alignment::Center)
            .style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .render(jump_area, buf);
    }

    let composer_columns =
        Layout::horizontal([Constraint::Min(10), Constraint::Length(10)]).split(rows[3]);
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
    let button_color = if app.sending {
        Color::DarkGray
    } else {
        Color::Rgb(40, 210, 130)
    };
    let button = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(button_color));
    let button_inner = button.inner(composer_columns[1]);
    button.render(composer_columns[1], buf);
    let symbol_style = Style::default()
        .fg(button_color)
        .add_modifier(Modifier::BOLD);
    let symbol_area = Rect::new(
        button_inner.x,
        button_inner.y + button_inner.height.saturating_sub(1) / 2,
        button_inner.width,
        1.min(button_inner.height),
    );
    Paragraph::new(Span::styled(
        if app.sending { "…" } else { "⌲" },
        symbol_style,
    ))
    .alignment(Alignment::Center)
    .render(symbol_area, buf);

    if let Some(footer) = rows.last() {
        Paragraph::new("Enter: send · Shift+Enter: newline · PgUp/PgDn: scroll · End: latest")
            .alignment(Alignment::Center)
            .fg(Color::DarkGray)
            .render(*footer, buf);
    }
}

fn bubble_size(text: &str, sender: &str, time: &str, area_width: u16) -> (u16, u16) {
    let available_width = area_width.saturating_sub(1).max(1);
    let max_width = available_width.saturating_mul(3).div_ceil(4).max(1);
    let title_width = sender.chars().count() + time.chars().count() + 5;
    let longest_line = text
        .lines()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(1);
    let desired = u16::try_from(longest_line.max(title_width).saturating_add(4))
        .unwrap_or(max_width)
        .clamp(12.min(available_width), max_width.min(available_width));
    let content_width = usize::from(desired.saturating_sub(4).max(1));
    let content_height = text
        .split('\n')
        .map(|line| line.chars().count().max(1).div_ceil(content_width))
        .sum::<usize>()
        .max(1);
    let height = u16::try_from(content_height.saturating_add(2)).unwrap_or(u16::MAX);
    (height, desired)
}

#[allow(clippy::too_many_arguments)]
fn render_message_bubbles(
    app: &App,
    peer: &str,
    area: Rect,
    buf: &mut Buffer,
    bubble_specs: &[(u16, u16)],
    visible_start: usize,
    visible_end: usize,
    gap: usize,
    visible_height: usize,
) {
    let mut y = area.bottom().saturating_sub(
        u16::try_from(visible_height)
            .unwrap_or(area.height)
            .min(area.height),
    );
    for (index, &(height, width)) in bubble_specs
        .iter()
        .enumerate()
        .take(visible_end)
        .skip(visible_start)
    {
        let message = &app.messages[index];
        let height = height.min(area.bottom().saturating_sub(y));
        if height == 0 {
            continue;
        }
        let x = if message.outgoing {
            area.right()
                .saturating_sub(3)
                .saturating_sub(width)
                .max(area.x)
        } else {
            area.x
        };
        let bubble_area = Rect::new(x, y, width, height);
        let time = message.timestamp.get(11..16).unwrap_or("");
        let sender = if message.outgoing { "You" } else { peer };
        let color = if message.outgoing {
            Color::Rgb(40, 210, 130)
        } else {
            Color::Rgb(70, 160, 255)
        };
        let bubble = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::DarkGray))
            .padding(Padding::horizontal(1))
            .title(Line::from(Span::styled(
                format!(" {sender} · {time} "),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )));
        Paragraph::new(message.text.as_str())
            .block(bubble)
            .wrap(Wrap { trim: false })
            .render(bubble_area, buf);
        y = y
            .saturating_add(height)
            .saturating_add(u16::try_from(gap).unwrap_or(0));
    }
}

fn render_chat_scrollbar(
    area: Rect,
    buf: &mut Buffer,
    content_height: usize,
    viewport_height: usize,
    top_line: usize,
) {
    let track_height = usize::from(area.height);
    if track_height == 0 || content_height <= viewport_height {
        return;
    }
    let minimum_thumb = 2.min(track_height);
    let thumb_height = viewport_height
        .saturating_mul(track_height)
        .div_ceil(content_height)
        .max(minimum_thumb)
        .min(track_height);
    let max_top = content_height.saturating_sub(viewport_height);
    let travel = track_height.saturating_sub(thumb_height);
    let thumb_offset = top_line
        .min(max_top)
        .saturating_mul(travel)
        .checked_div(max_top)
        .unwrap_or(0);
    let x = area.right().saturating_sub(1);
    for offset in 0..track_height {
        let y = area.y.saturating_add(offset as u16);
        let thumb = offset >= thumb_offset && offset < thumb_offset + thumb_height;
        buf[(x, y)]
            .set_symbol(if thumb { "█" } else { "│" })
            .set_style(Style::default().fg(if thumb {
                Color::Cyan
            } else {
                Color::Rgb(45, 50, 60)
            }));
    }
}

fn render_invitation(app: &App, area: Rect, buf: &mut Buffer) {
    let block = panel("Invite contact").padding(Padding::new(2, 2, 1, 1));
    let inner = block.inner(area);
    block.render(area, buf);

    if app.invitation_loading {
        Paragraph::new("Creating a secure one-time invitation via SimpleX…")
            .alignment(Alignment::Center)
            .fg(Color::Cyan)
            .render(inner, buf);
        return;
    }
    if let Some(error) = &app.invitation_error {
        Paragraph::new(format!(
            "Could not create invitation:\n\n{error}\n\nPress r to retry."
        ))
        .fg(Color::Red)
        .wrap(Wrap { trim: false })
        .render(inner, buf);
        return;
    }
    let Some(link) = &app.invitation_link else {
        Paragraph::new("Press Enter to create a one-time invitation link.")
            .alignment(Alignment::Center)
            .render(inner, buf);
        return;
    };

    let rows = Layout::vertical([
        Constraint::Min(10),
        Constraint::Length(1),
        Constraint::Length(6),
        Constraint::Length(1),
    ])
    .split(inner);
    if let Ok(code) = QrCode::new(link.as_bytes()) {
        QrCodeWidget::new(code)
            .scaling(Scaling::Max)
            .colors(Colors::Inverted)
            .render(rows[0], buf);
    }
    Paragraph::new("Scan with SimpleX Chat")
        .alignment(Alignment::Center)
        .fg(Color::Cyan)
        .render(rows[1], buf);
    Paragraph::new(link.as_str())
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false })
        .render(rows[2], buf);
    Paragraph::new("One-time link · r: create a new invitation")
        .alignment(Alignment::Center)
        .fg(Color::DarkGray)
        .render(rows[3], buf);
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
            "Notification sound\n{}\n\nEnter/Space: toggle sound\n\nWhen enabled, incoming messages ring the terminal bell.",
            enabled(app.preferences.notification_sound)
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
        4 => format!(
            "Defaults for new contacts\n\nDisappearing messages  {}   [d]\nFull deletion          {}   [x]\nMessage reactions      {}   [r]\nVoice messages         {}   [v]\nFiles and media        {}   [f]\nAudio/video calls      Disabled (fixed)\n\nThese preferences are negotiated by SimpleX. Their protocol events are not displayed as chat messages.",
            enabled(app.chat_features.disappearing_messages),
            enabled(app.chat_features.full_deletion),
            enabled(app.chat_features.reactions),
            enabled(app.chat_features.voice_messages),
            enabled(app.chat_features.files_and_media),
        ),
        5 => {
            let servers = if app.smp_servers.is_empty() {
                "No SMP servers loaded.".into()
            } else {
                app.smp_servers
                    .iter()
                    .map(|server| server.split('@').nth(1).unwrap_or(server).replace(',', "\n    onion: "))
                    .collect::<Vec<_>>()
                    .join("\n\n")
            };
            format!(
                "Servers configured for the active profile\n\n{servers}\n\nNew profiles use the official SimpleX preset. Invitations use this profile-specific server configuration."
            )
        }
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
