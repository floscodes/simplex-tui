use std::{
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::Duration,
};

use crate::{
    Config,
    ffi::{SimplexApi, SimplexPaths},
    model::{self, ChatRef, ServerProtocol, SimplexEvent},
};

#[derive(Debug)]
pub enum SimplexCommand {
    LoadChat(ChatRef),
    MarkChatRead(ChatRef),
    SendMessage {
        chat_ref: ChatRef,
        text: String,
    },
    SendReaction {
        chat_ref: ChatRef,
        item_id: i64,
        emoji: String,
    },
    ReceiveFile {
        file_id: i64,
        file_name: String,
    },
    CancelFile {
        file_id: i64,
    },
    ActivateProfile(i64),
    CreateProfile(String),
    DeleteProfile(i64),
    SetAutoDelete {
        user_id: i64,
        seconds: i64,
    },
    LoadChatDeletion {
        chat_ref: ChatRef,
    },
    SetChatDeletion {
        user_id: i64,
        chat_ref: ChatRef,
        seconds: Option<i64>,
    },
    LoadConversationFeatures {
        chat_ref: ChatRef,
    },
    SetConversationFeature {
        chat_ref: ChatRef,
        feature: ChatFeature,
        enabled: bool,
    },
    DeleteChat {
        user_id: i64,
        chat_ref: ChatRef,
        mode: ChatDeleteMode,
    },
    CreateInvitation {
        user_id: i64,
    },
    ConnectInvitation {
        user_id: i64,
        link: String,
    },
    SetChatFeature {
        user_id: i64,
        feature: ChatFeature,
        enabled: bool,
    },
    SetServerEnabled {
        user_id: i64,
        protocol: ServerProtocol,
        address: String,
        enabled: bool,
    },
    AddServer {
        user_id: i64,
        protocol: ServerProtocol,
        address: String,
    },
}

#[derive(Clone, Copy, Debug)]
pub enum ChatFeature {
    FullDeletion,
    Reactions,
    VoiceMessages,
    FilesAndMedia,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChatDeleteMode {
    Conversation,
    Contact,
    BlockContact,
}

struct DeletedProfileState {
    profiles: Vec<model::Profile>,
    active_user: Option<model::User>,
    chats: Vec<model::ChatSummary>,
}

pub fn spawn(
    api: Arc<SimplexApi>,
    config: Config,
    sender: Sender<SimplexEvent>,
) -> Sender<SimplexCommand> {
    let (command_sender, commands) = mpsc::channel();
    thread::Builder::new()
        .name("simplex-core".into())
        .spawn(move || {
            if let Err(error) = run(&api, &config, &sender, &commands) {
                let _ = sender.send(SimplexEvent::Failed(error));
            }
        })
        .expect("failed to spawn SimpleX worker");
    command_sender
}

fn run(
    api: &Arc<SimplexApi>,
    config: &Config,
    sender: &Sender<SimplexEvent>,
    commands: &Receiver<SimplexCommand>,
) -> Result<(), String> {
    let paths = SimplexPaths::at(&config.data_directory);
    paths
        .create()
        .map_err(|e| format!("cannot create {}: {e}", paths.root.display()))?;
    let (controller, migration) = api
        .open(&paths.database_prefix, "", "yesUp")
        .map_err(|e| e.to_string())?;
    if migration.get("error").is_some() {
        return Err(format!("database migration failed: {migration}"));
    }

    let profiles = load_profiles(&controller)?;
    sender
        .send(SimplexEvent::ProfilesLoaded(profiles))
        .map_err(|e| e.to_string())?;

    let Some(user) = model::active_user(&controller.command("/u").map_err(|e| e.to_string())?)?
    else {
        sender
            .send(SimplexEvent::NoActiveUser)
            .map_err(|e| e.to_string())?;
        return command_loop(controller, &config.download_directory, sender, commands);
    };
    controller.command("/_start").map_err(|e| e.to_string())?;
    send_auto_delete(&controller, sender, user.id)?;
    send_servers(&controller, sender, user.id)?;
    load_chat_features(&controller, sender, user.id, true)?;
    let chats = load_chats(&controller, user.id)?;
    sender
        .send(SimplexEvent::Ready { user, chats })
        .map_err(|e| e.to_string())?;

    command_loop(controller, &config.download_directory, sender, commands)
}

fn command_loop(
    controller: crate::ffi::SimplexController,
    download_directory: &std::path::Path,
    sender: &Sender<SimplexEvent>,
    commands: &Receiver<SimplexCommand>,
) -> Result<(), String> {
    loop {
        while let Ok(command) = commands.try_recv() {
            match command {
                SimplexCommand::LoadChat(requested_ref) => {
                    let response =
                        controller.command(&format!("/_get chat {} count=100", requested_ref.0));
                    let event = match response
                        .map_err(|e| e.to_string())
                        .and_then(|v| model::chat_messages(&v))
                    {
                        Ok((chat_ref, messages)) => SimplexEvent::ChatLoaded { chat_ref, messages },
                        Err(error) => SimplexEvent::ChatLoadFailed {
                            chat_ref: requested_ref.clone(),
                            error,
                        },
                    };
                    let loaded = matches!(event, SimplexEvent::ChatLoaded { .. });
                    sender.send(event).map_err(|e| e.to_string())?;
                    if loaded {
                        mark_chat_read(&controller, sender, &requested_ref)?;
                    }
                }
                SimplexCommand::MarkChatRead(chat_ref) => {
                    mark_chat_read(&controller, sender, &chat_ref)?;
                }
                SimplexCommand::SendMessage { chat_ref, text } => {
                    let command = if let Some(id) = chat_ref.0.strip_prefix('*') {
                        format!("/_create *{id} text {text}")
                    } else if chat_ref.0.starts_with('@') || chat_ref.0.starts_with('#') {
                        format!(
                            "/_send {} live=off ttl=default sign=off text {text}",
                            chat_ref.0
                        )
                    } else {
                        sender
                            .send(SimplexEvent::MessageSendFailed {
                                chat_ref,
                                error: "messages cannot be sent to this chat type".into(),
                            })
                            .map_err(|e| e.to_string())?;
                        continue;
                    };
                    match controller.command(&command).map_err(|e| e.to_string()) {
                        Ok(response)
                            if response
                                .pointer("/result/type")
                                .and_then(serde_json::Value::as_str)
                                == Some("newChatItems") =>
                        {
                            for (item_ref, message) in model::new_messages(&response) {
                                sender
                                    .send(SimplexEvent::MessageReceived {
                                        chat_ref: item_ref,
                                        message,
                                    })
                                    .map_err(|e| e.to_string())?;
                            }
                            sender
                                .send(SimplexEvent::MessageSent { chat_ref, text })
                                .map_err(|e| e.to_string())?;
                        }
                        Ok(response) => sender
                            .send(SimplexEvent::MessageSendFailed {
                                chat_ref,
                                error: format!("send failed: {response}"),
                            })
                            .map_err(|e| e.to_string())?,
                        Err(error) => sender
                            .send(SimplexEvent::MessageSendFailed { chat_ref, error })
                            .map_err(|e| e.to_string())?,
                    }
                }
                SimplexCommand::SendReaction {
                    chat_ref,
                    item_id,
                    emoji,
                } => {
                    let payload = serde_json::json!({"type": "emoji", "emoji": emoji});
                    let response = controller
                        .command(&format!("/_reaction {} {item_id} on {payload}", chat_ref.0))
                        .map_err(|e| e.to_string())?;
                    send_reaction_change(sender, &response)?;
                }
                SimplexCommand::ReceiveFile { file_id, file_name } => {
                    match receive_file(&controller, download_directory, file_id, &file_name) {
                        Ok((path, response)) => {
                            sender
                                .send(SimplexEvent::FileDownloadStarted {
                                    file_id,
                                    path: path.to_string_lossy().into_owned(),
                                })
                                .map_err(|e| e.to_string())?;
                            if let Some((chat_ref, message)) = model::file_update(&response) {
                                sender
                                    .send(SimplexEvent::FileUpdated { chat_ref, message })
                                    .map_err(|e| e.to_string())?;
                            }
                        }
                        Err(error) => sender
                            .send(SimplexEvent::FileDownloadFailed { file_id, error })
                            .map_err(|e| e.to_string())?,
                    }
                }
                SimplexCommand::CancelFile { file_id } => match cancel_file(&controller, file_id) {
                    Ok(response) => {
                        sender
                            .send(SimplexEvent::FileDownloadCancelled { file_id })
                            .map_err(|e| e.to_string())?;
                        if let Some((chat_ref, message)) = model::file_update(&response) {
                            sender
                                .send(SimplexEvent::FileUpdated { chat_ref, message })
                                .map_err(|e| e.to_string())?;
                        }
                    }
                    Err(error) => sender
                        .send(SimplexEvent::FileDownloadFailed { file_id, error })
                        .map_err(|e| e.to_string())?,
                },
                SimplexCommand::ActivateProfile(user_id) => {
                    let result = (|| {
                        let response = controller
                            .command(&format!("/_user {user_id}"))
                            .map_err(|e| e.to_string())?;
                        let user = model::active_user(&response)?
                            .ok_or("SimpleX did not activate the selected profile")?;
                        controller.command("/_start").map_err(|e| e.to_string())?;
                        send_auto_delete(&controller, sender, user.id)?;
                        send_servers(&controller, sender, user.id)?;
                        load_chat_features(&controller, sender, user.id, true)?;
                        let chats = load_chats(&controller, user.id)?;
                        Ok::<_, String>(SimplexEvent::ProfileActivated { user, chats })
                    })();
                    sender
                        .send(result.unwrap_or_else(SimplexEvent::Failed))
                        .map_err(|e| e.to_string())?;
                }
                SimplexCommand::CreateProfile(name) => {
                    let result = (|| {
                        let escaped = serde_json::to_string(&name).map_err(|e| e.to_string())?;
                        let payload = format!(
                            "{{\"profile\":{{\"displayName\":{escaped},\"fullName\":\"\"}},\"pastTimestamp\":false}}"
                        );
                        let response = controller
                            .command(&format!("/_create user {payload}"))
                            .map_err(|e| e.to_string())?;
                        let user = model::active_user(&response)?
                            .ok_or("SimpleX did not create the profile")?;
                        controller.command("/_start").map_err(|e| e.to_string())?;
                        send_auto_delete(&controller, sender, user.id)?;
                        send_servers(&controller, sender, user.id)?;
                        load_chat_features(&controller, sender, user.id, true)?;
                        let profiles = load_profiles(&controller)?;
                        let chats = load_chats(&controller, user.id)?;
                        Ok::<_, String>(SimplexEvent::ProfileCreated {
                            user,
                            profiles,
                            chats,
                        })
                    })();
                    sender
                        .send(result.unwrap_or_else(SimplexEvent::Failed))
                        .map_err(|e| e.to_string())?;
                }
                SimplexCommand::DeleteProfile(user_id) => {
                    let result = delete_profile(&controller, user_id);
                    sender
                        .send(match result {
                            Ok(state) => SimplexEvent::ProfileDeleted {
                                profiles: state.profiles,
                                active_user: state.active_user,
                                chats: state.chats,
                            },
                            Err(error) => SimplexEvent::ProfileDeleteFailed(error),
                        })
                        .map_err(|e| e.to_string())?;
                }
                SimplexCommand::SetAutoDelete { user_id, seconds } => {
                    let result = update_global_deletion(&controller, user_id, seconds);
                    sender
                        .send(match result {
                            Ok(seconds) => SimplexEvent::AutoDeleteChanged(seconds),
                            Err(error) => SimplexEvent::AutoDeleteFailed(error),
                        })
                        .map_err(|e| e.to_string())?;
                }
                SimplexCommand::LoadChatDeletion { chat_ref } => {
                    let result = load_chat_deletion(&controller, &chat_ref);
                    sender
                        .send(match result {
                            Ok(settings) => SimplexEvent::ChatDeletionLoaded { chat_ref, settings },
                            Err(error) => SimplexEvent::ChatDeletionFailed(error),
                        })
                        .map_err(|e| e.to_string())?;
                }
                SimplexCommand::SetChatDeletion {
                    user_id,
                    chat_ref,
                    seconds,
                } => {
                    let result = update_chat_deletion(&controller, user_id, &chat_ref, seconds);
                    sender
                        .send(match result {
                            Ok(settings) => {
                                SimplexEvent::ChatDeletionChanged { chat_ref, settings }
                            }
                            Err(error) => SimplexEvent::ChatDeletionFailed(error),
                        })
                        .map_err(|e| e.to_string())?;
                }
                SimplexCommand::LoadConversationFeatures { chat_ref } => {
                    let event = load_conversation_features(&controller, &chat_ref)
                        .map(|features| SimplexEvent::ConversationFeaturesLoaded {
                            chat_ref,
                            features,
                        })
                        .unwrap_or_else(SimplexEvent::ConversationFeaturesFailed);
                    sender.send(event).map_err(|e| e.to_string())?;
                }
                SimplexCommand::SetConversationFeature {
                    chat_ref,
                    feature,
                    enabled,
                } => {
                    let event =
                        update_conversation_feature(&controller, &chat_ref, feature, enabled)
                            .map(|features| SimplexEvent::ConversationFeaturesChanged {
                                chat_ref,
                                features,
                            })
                            .unwrap_or_else(SimplexEvent::ConversationFeaturesFailed);
                    sender.send(event).map_err(|e| e.to_string())?;
                }
                SimplexCommand::DeleteChat {
                    user_id,
                    chat_ref,
                    mode,
                } => {
                    let event = delete_chat(&controller, user_id, &chat_ref, mode)
                        .map(|chats| SimplexEvent::ChatDeleted { chat_ref, chats })
                        .unwrap_or_else(SimplexEvent::ChatDeleteFailed);
                    sender.send(event).map_err(|e| e.to_string())?;
                }
                SimplexCommand::CreateInvitation { user_id } => {
                    let event = controller
                        .command(&format!("/_connect {user_id} incognito=off"))
                        .map_err(|e| e.to_string())
                        .and_then(|value| model::invitation_link(&value))
                        .map(SimplexEvent::InvitationCreated)
                        .unwrap_or_else(SimplexEvent::InvitationFailed);
                    sender.send(event).map_err(|e| e.to_string())?;
                }
                SimplexCommand::ConnectInvitation { user_id, link } => {
                    let event = validate_connection_link(&link)
                        .and_then(|link| {
                            controller
                                .command(&format!("/_connect {user_id} incognito=off {link}"))
                                .map_err(|e| e.to_string())
                        })
                        .and_then(|value| model::connection_started(&value))
                        .map(|()| SimplexEvent::ConnectionStarted)
                        .unwrap_or_else(SimplexEvent::ConnectionFailed);
                    sender.send(event).map_err(|e| e.to_string())?;
                }
                SimplexCommand::SetChatFeature {
                    user_id,
                    feature,
                    enabled,
                } => {
                    let result = update_chat_feature(&controller, user_id, feature, enabled);
                    match result {
                        Ok(features) => {
                            sender
                                .send(SimplexEvent::ChatFeaturesLoaded(features))
                                .map_err(|e| e.to_string())?;
                            sender
                                .send(SimplexEvent::SettingChanged("Chat feature updated".into()))
                                .map_err(|e| e.to_string())?;
                        }
                        Err(error) => sender
                            .send(SimplexEvent::SettingChanged(format!(
                                "Could not update chat feature: {error}"
                            )))
                            .map_err(|e| e.to_string())?,
                    }
                }
                SimplexCommand::SetServerEnabled {
                    user_id,
                    protocol,
                    address,
                    enabled,
                } => {
                    let result =
                        set_server_enabled(&controller, user_id, protocol, &address, enabled)
                            .and_then(|()| load_servers(&controller, user_id));
                    sender
                        .send(match result {
                            Ok(servers) => SimplexEvent::ServersLoaded(servers),
                            Err(error) => SimplexEvent::ServersUpdateFailed(error),
                        })
                        .map_err(|e| e.to_string())?;
                }
                SimplexCommand::AddServer {
                    user_id,
                    protocol,
                    address,
                } => {
                    let result = add_server(&controller, user_id, protocol, &address)
                        .and_then(|()| load_servers(&controller, user_id));
                    sender
                        .send(match result {
                            Ok(servers) => SimplexEvent::ServersLoaded(servers),
                            Err(error) => SimplexEvent::ServersUpdateFailed(error),
                        })
                        .map_err(|e| e.to_string())?;
                }
            }
        }
        if let Some(value) = controller
            .recv(Duration::from_millis(200))
            .map_err(|e| e.to_string())?
        {
            send_reaction_change(sender, &value)?;
            if let Some((chat_ref, message)) = model::file_update(&value) {
                sender
                    .send(SimplexEvent::FileUpdated { chat_ref, message })
                    .map_err(|e| e.to_string())?;
            }
            if let Some((user_id, chat_ref)) = model::connected_contact(&value) {
                let chats = load_chats(&controller, user_id)?;
                sender
                    .send(SimplexEvent::ContactConnected { chats, chat_ref })
                    .map_err(|e| e.to_string())?;
            }
            for (chat_ref, message) in model::new_messages(&value) {
                sender
                    .send(SimplexEvent::MessageReceived { chat_ref, message })
                    .map_err(|e| e.to_string())?;
            }
        }
    }
}

fn validate_connection_link(link: &str) -> Result<&str, String> {
    let link = link.trim();
    if link.is_empty() {
        return Err("invitation link is empty".into());
    }
    if link.chars().any(char::is_whitespace) {
        return Err("invitation link contains whitespace".into());
    }
    Ok(link)
}

fn receive_file(
    controller: &crate::ffi::SimplexController,
    directory: &std::path::Path,
    file_id: i64,
    file_name: &str,
) -> Result<(std::path::PathBuf, serde_json::Value), String> {
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
    let safe_name = std::path::Path::new(file_name)
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .filter(|name| !name.is_empty() && *name != "." && *name != "..")
        .unwrap_or("simplex-file");
    let path = unused_download_path(directory, safe_name);
    let response = controller
        .command(&format!(
            "/freceive {file_id} approved_relays=on {}",
            path.display()
        ))
        .map_err(|error| error.to_string())?;
    if response
        .pointer("/result/type")
        .and_then(serde_json::Value::as_str)
        != Some("rcvFileAccepted")
    {
        return Err(format!("file download was not accepted: {response}"));
    }
    Ok((path, response))
}

fn cancel_file(
    controller: &crate::ffi::SimplexController,
    file_id: i64,
) -> Result<serde_json::Value, String> {
    let response = controller
        .command(&format!("/fcancel {file_id}"))
        .map_err(|error| error.to_string())?;
    if response
        .pointer("/result/type")
        .and_then(serde_json::Value::as_str)
        != Some("rcvFileCancelled")
    {
        return Err(format!("file download was not cancelled: {response}"));
    }
    Ok(response)
}

fn unused_download_path(directory: &std::path::Path, name: &str) -> std::path::PathBuf {
    let original = directory.join(name);
    if !original.exists() {
        return original;
    }
    let path = std::path::Path::new(name);
    let stem = path
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("file");
    let extension = path.extension().and_then(std::ffi::OsStr::to_str);
    for number in 1.. {
        let candidate = match extension {
            Some(extension) => directory.join(format!("{stem} ({number}).{extension}")),
            None => directory.join(format!("{stem} ({number})")),
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

fn send_reaction_change(
    sender: &Sender<SimplexEvent>,
    value: &serde_json::Value,
) -> Result<(), String> {
    if let Some((chat_ref, item_id, emoji, added, user_reacted)) = model::reaction_change(value) {
        sender
            .send(SimplexEvent::ReactionChanged {
                chat_ref,
                item_id,
                emoji,
                added,
                user_reacted,
            })
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn delete_profile(
    controller: &crate::ffi::SimplexController,
    user_id: i64,
) -> Result<DeletedProfileState, String> {
    let profiles = load_profiles(controller)?;
    if profiles
        .iter()
        .any(|profile| profile.id == user_id && profile.active)
        && let Some(other) = profiles.iter().find(|profile| profile.id != user_id)
    {
        let response = controller
            .command(&format!("/_user {}", other.id))
            .map_err(|e| e.to_string())?;
        model::active_user(&response)?.ok_or("SimpleX did not activate the replacement profile")?;
    }
    let response = controller
        .command(&format!("/_delete user {user_id} del_smp=on"))
        .map_err(|e| e.to_string())?;
    ensure_ok(&response, "profile deletion")?;
    let profiles = load_profiles(controller)?;
    let active_user = model::active_user(&controller.command("/u").map_err(|e| e.to_string())?)?;
    let chats = if let Some(user) = &active_user {
        load_chats(controller, user.id)?
    } else {
        Vec::new()
    };
    Ok(DeletedProfileState {
        profiles,
        active_user,
        chats,
    })
}

fn mark_chat_read(
    controller: &crate::ffi::SimplexController,
    sender: &Sender<SimplexEvent>,
    chat_ref: &ChatRef,
) -> Result<(), String> {
    let response = controller
        .command(&format!("/_read chat {}", chat_ref.0))
        .map_err(|e| e.to_string())?;
    ensure_ok(&response, "mark chat as read")?;
    sender
        .send(SimplexEvent::ChatMarkedRead(chat_ref.clone()))
        .map_err(|e| e.to_string())
}

fn load_chat_features(
    controller: &crate::ffi::SimplexController,
    sender: &Sender<SimplexEvent>,
    user_id: i64,
    enforce_calls_disabled: bool,
) -> Result<(), String> {
    let response = controller.command("/profile").map_err(|e| e.to_string())?;
    let (mut profile, features) = model::profile_and_features(&response)?;
    if enforce_calls_disabled && preference_allow(&profile, "calls") != Some("no") {
        set_profile_preference(&mut profile, "calls", false)?;
        update_profile(controller, user_id, &profile)?;
    }
    sender
        .send(SimplexEvent::ChatFeaturesLoaded(features))
        .map_err(|e| e.to_string())
}

fn update_chat_feature(
    controller: &crate::ffi::SimplexController,
    user_id: i64,
    feature: ChatFeature,
    enabled: bool,
) -> Result<model::ChatFeatures, String> {
    let response = controller.command("/profile").map_err(|e| e.to_string())?;
    let (mut profile, _) = model::profile_and_features(&response)?;
    let name = match feature {
        ChatFeature::FullDeletion => "fullDelete",
        ChatFeature::Reactions => "reactions",
        ChatFeature::VoiceMessages => "voice",
        ChatFeature::FilesAndMedia => "files",
    };
    set_profile_preference(&mut profile, name, enabled)?;
    set_profile_preference(&mut profile, "calls", false)?;
    update_profile(controller, user_id, &profile)?;
    let synthetic = serde_json::json!({"result": {"type": "userProfile", "profile": profile}});
    model::profile_and_features(&synthetic).map(|(_, features)| features)
}

fn update_global_deletion(
    controller: &crate::ffi::SimplexController,
    user_id: i64,
    seconds: i64,
) -> Result<i64, String> {
    let response = controller.command("/profile").map_err(|e| e.to_string())?;
    let (mut profile, _) = model::profile_and_features(&response)?;
    set_timed_preference(&mut profile, Some(seconds))?;
    set_profile_preference(&mut profile, "calls", false)?;
    update_profile(controller, user_id, &profile)?;
    let response = controller
        .command(&format!("/_ttl {user_id} {seconds}"))
        .map_err(|e| e.to_string())?;
    ensure_ok(&response, "automatic deletion")?;
    Ok(seconds)
}

fn load_chat_deletion(
    controller: &crate::ffi::SimplexController,
    chat_ref: &ChatRef,
) -> Result<model::ChatDeletionSettings, String> {
    let response = controller
        .command(&format!("/_get chat {} count=1", chat_ref.0))
        .map_err(|e| e.to_string())?;
    let info = response
        .pointer("/result/chat/chatInfo")
        .ok_or("chat response has no chatInfo")?;
    let (entity, timed_pointer) = match info.get("type").and_then(serde_json::Value::as_str) {
        Some("direct") => (
            info.get("contact").ok_or("direct chat has no contact")?,
            "/userPreferences/timedMessages",
        ),
        Some("group") => (
            info.get("groupInfo").ok_or("group chat has no groupInfo")?,
            "/groupProfile/groupPreferences/timedMessages",
        ),
        _ => return Err("chat deletion settings are unsupported for this chat type".into()),
    };
    let local_ttl = entity
        .get("chatItemTTL")
        .and_then(serde_json::Value::as_i64);
    let timed = entity.pointer(timed_pointer);
    let enabled = timed
        .and_then(|value| value.get("allow").or_else(|| value.get("enable")))
        .and_then(serde_json::Value::as_str);
    let disappearing_ttl = match enabled {
        Some("no" | "off") => Some(0),
        Some("yes" | "always" | "on") => timed
            .and_then(|value| value.get("ttl"))
            .and_then(serde_json::Value::as_i64)
            .or(Some(0)),
        _ => None,
    };
    Ok(model::ChatDeletionSettings {
        local_ttl,
        disappearing_ttl,
    })
}

fn update_chat_deletion(
    controller: &crate::ffi::SimplexController,
    user_id: i64,
    chat_ref: &ChatRef,
    seconds: Option<i64>,
) -> Result<model::ChatDeletionSettings, String> {
    let response = controller
        .command(&format!("/_get chat {} count=1", chat_ref.0))
        .map_err(|e| e.to_string())?;
    let info = response
        .pointer("/result/chat/chatInfo")
        .ok_or("chat response has no chatInfo")?;
    match info.get("type").and_then(serde_json::Value::as_str) {
        Some("direct") => {
            let mut preferences = info
                .pointer("/contact/userPreferences")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            set_timed_preference(&mut preferences, seconds)?;
            let response = controller
                .command(&format!("/_set prefs {} {preferences}", chat_ref.0))
                .map_err(|e| e.to_string())?;
            ensure_ok(&response, "contact disappearing messages")?;
        }
        Some("group") => {
            let mut profile = info
                .pointer("/groupInfo/groupProfile")
                .cloned()
                .ok_or("group chat has no group profile")?;
            set_group_timed_preference(&mut profile, seconds)?;
            let response = controller
                .command(&format!("/_group_profile {} {profile}", chat_ref.0))
                .map_err(|e| e.to_string())?;
            ensure_ok(&response, "group disappearing messages")?;
        }
        _ => return Err("chat deletion settings are unsupported for this chat type".into()),
    }
    let ttl = seconds.map_or_else(|| "default".to_owned(), |value| value.to_string());
    let response = controller
        .command(&format!("/_ttl {user_id} {} {ttl}", chat_ref.0))
        .map_err(|e| e.to_string())?;
    ensure_ok(&response, "local chat deletion")?;
    load_chat_deletion(controller, chat_ref)
}

fn load_conversation_features(
    controller: &crate::ffi::SimplexController,
    chat_ref: &ChatRef,
) -> Result<model::ChatFeatures, String> {
    let response = controller
        .command(&format!("/_get chat {} count=1", chat_ref.0))
        .map_err(|e| e.to_string())?;
    let info = response
        .pointer("/result/chat/chatInfo")
        .ok_or("chat response has no chatInfo")?;
    let (_, defaults) =
        model::profile_and_features(&controller.command("/profile").map_err(|e| e.to_string())?)?;
    match info.get("type").and_then(serde_json::Value::as_str) {
        Some("direct") => Ok(features_from_preferences(
            info.pointer("/contact/userPreferences"),
            defaults,
            false,
        )),
        Some("group") => Ok(features_from_preferences(
            info.pointer("/groupInfo/groupProfile/groupPreferences"),
            defaults,
            true,
        )),
        _ => Err("chat feature settings are unsupported for this chat type".into()),
    }
}

fn update_conversation_feature(
    controller: &crate::ffi::SimplexController,
    chat_ref: &ChatRef,
    feature: ChatFeature,
    enabled: bool,
) -> Result<model::ChatFeatures, String> {
    let response = controller
        .command(&format!("/_get chat {} count=1", chat_ref.0))
        .map_err(|e| e.to_string())?;
    let info = response
        .pointer("/result/chat/chatInfo")
        .ok_or("chat response has no chatInfo")?;
    let name = feature_name(feature);
    match info.get("type").and_then(serde_json::Value::as_str) {
        Some("direct") => {
            let mut preferences = info
                .pointer("/contact/userPreferences")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            set_contact_feature(&mut preferences, name, enabled)?;
            set_contact_feature(&mut preferences, "calls", false)?;
            let response = controller
                .command(&format!("/_set prefs {} {preferences}", chat_ref.0))
                .map_err(|e| e.to_string())?;
            ensure_ok(&response, "contact feature")?;
        }
        Some("group") => {
            let mut profile = info
                .pointer("/groupInfo/groupProfile")
                .cloned()
                .ok_or("group chat has no group profile")?;
            set_group_feature(&mut profile, name, enabled)?;
            set_group_feature(&mut profile, "calls", false)?;
            let response = controller
                .command(&format!("/_group_profile {} {profile}", chat_ref.0))
                .map_err(|e| e.to_string())?;
            ensure_ok(&response, "group feature")?;
        }
        _ => return Err("chat feature settings are unsupported for this chat type".into()),
    }
    load_conversation_features(controller, chat_ref)
}

fn delete_chat(
    controller: &crate::ffi::SimplexController,
    user_id: i64,
    chat_ref: &ChatRef,
    mode: ChatDeleteMode,
) -> Result<Vec<model::ChatSummary>, String> {
    let suffix = match mode {
        ChatDeleteMode::Conversation => "messages",
        ChatDeleteMode::Contact if chat_ref.0.starts_with('@') => "full notify=on",
        ChatDeleteMode::BlockContact if chat_ref.0.starts_with('@') => "full notify=off",
        ChatDeleteMode::Contact | ChatDeleteMode::BlockContact => {
            return Err("contact actions are only available for direct chats".into());
        }
    };
    let response = controller
        .command(&format!("/_delete {} {suffix}", chat_ref.0))
        .map_err(|e| e.to_string())?;
    ensure_ok(&response, "chat deletion")?;
    load_chats(controller, user_id)
}

fn feature_name(feature: ChatFeature) -> &'static str {
    match feature {
        ChatFeature::FullDeletion => "fullDelete",
        ChatFeature::Reactions => "reactions",
        ChatFeature::VoiceMessages => "voice",
        ChatFeature::FilesAndMedia => "files",
    }
}

fn features_from_preferences(
    preferences: Option<&serde_json::Value>,
    defaults: model::ChatFeatures,
    group: bool,
) -> model::ChatFeatures {
    let enabled = |name: &str, default: bool| {
        let value = preferences
            .and_then(|prefs| prefs.get(name))
            .and_then(|pref| pref.get(if group { "enable" } else { "allow" }))
            .and_then(serde_json::Value::as_str);
        match value {
            Some("no" | "off") => false,
            Some("yes" | "always" | "on") => true,
            _ => default,
        }
    };
    model::ChatFeatures {
        disappearing_messages: defaults.disappearing_messages,
        full_deletion: enabled("fullDelete", defaults.full_deletion),
        reactions: enabled("reactions", defaults.reactions),
        voice_messages: enabled("voice", defaults.voice_messages),
        files_and_media: enabled("files", defaults.files_and_media),
    }
}

fn set_contact_feature(
    preferences: &mut serde_json::Value,
    name: &str,
    enabled: bool,
) -> Result<(), String> {
    preferences
        .as_object_mut()
        .ok_or("contact preferences are not an object")?
        .insert(
            name.into(),
            serde_json::json!({"allow": if enabled { "yes" } else { "no" }}),
        );
    Ok(())
}

fn set_group_feature(
    profile: &mut serde_json::Value,
    name: &str,
    enabled: bool,
) -> Result<(), String> {
    profile
        .as_object_mut()
        .ok_or("group profile is not an object")?
        .entry("groupPreferences")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or("group preferences are not an object")?
        .insert(
            name.into(),
            serde_json::json!({"enable": if enabled { "on" } else { "off" }}),
        );
    Ok(())
}

fn set_timed_preference(
    object: &mut serde_json::Value,
    seconds: Option<i64>,
) -> Result<(), String> {
    let is_profile = object.get("displayName").is_some();
    let preferences = if is_profile {
        object
            .as_object_mut()
            .ok_or("profile is not an object")?
            .entry("preferences")
            .or_insert_with(|| serde_json::json!({}))
    } else {
        object
    };
    let preferences = preferences
        .as_object_mut()
        .ok_or("preferences are not an object")?;
    match seconds {
        None => {
            preferences.remove("timedMessages");
        }
        Some(seconds) => {
            preferences.insert(
                "timedMessages".into(),
                if seconds == 0 {
                    serde_json::json!({"allow": "no"})
                } else {
                    serde_json::json!({"allow": "yes", "ttl": seconds})
                },
            );
        }
    }
    Ok(())
}

fn set_group_timed_preference(
    profile: &mut serde_json::Value,
    seconds: Option<i64>,
) -> Result<(), String> {
    let profile = profile
        .as_object_mut()
        .ok_or("group profile is not an object")?;
    let preferences = profile
        .entry("groupPreferences")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or("group preferences are not an object")?;
    match seconds {
        None => {
            preferences.remove("timedMessages");
        }
        Some(seconds) => {
            preferences.insert(
                "timedMessages".into(),
                if seconds == 0 {
                    serde_json::json!({"enable": "off"})
                } else {
                    serde_json::json!({"enable": "on", "ttl": seconds})
                },
            );
        }
    }
    Ok(())
}

fn preference_allow<'a>(profile: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    profile
        .pointer(&format!("/preferences/{name}/allow"))
        .and_then(serde_json::Value::as_str)
}

fn set_profile_preference(
    profile: &mut serde_json::Value,
    name: &str,
    enabled: bool,
) -> Result<(), String> {
    let profile = profile
        .as_object_mut()
        .ok_or("profile response is not an object")?;
    if !profile
        .get("preferences")
        .is_some_and(|value| value.is_object())
    {
        profile.insert("preferences".into(), serde_json::json!({}));
    }
    let preferences = profile
        .get_mut("preferences")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or("profile preferences are not an object")?;
    let mut preference = preferences
        .get(name)
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    preference.insert(
        "allow".into(),
        serde_json::Value::String(if enabled { "yes" } else { "no" }.into()),
    );
    preferences.insert(name.into(), serde_json::Value::Object(preference));
    Ok(())
}

fn update_profile(
    controller: &crate::ffi::SimplexController,
    user_id: i64,
    profile: &serde_json::Value,
) -> Result<(), String> {
    let response = controller
        .command(&format!("/_profile {user_id} {profile}"))
        .map_err(|e| e.to_string())?;
    ensure_ok(&response, "chat preferences")
}

fn send_servers(
    controller: &crate::ffi::SimplexController,
    sender: &Sender<SimplexEvent>,
    user_id: i64,
) -> Result<(), String> {
    let servers = load_servers(controller, user_id)?;
    sender
        .send(SimplexEvent::ServersLoaded(servers))
        .map_err(|e| e.to_string())
}

fn load_servers(
    controller: &crate::ffi::SimplexController,
    user_id: i64,
) -> Result<Vec<model::ServerEntry>, String> {
    let response = controller
        .command(&format!("/_servers {user_id}"))
        .map_err(|e| e.to_string())?;
    model::server_entries(&response)
}

fn user_servers_json(
    controller: &crate::ffi::SimplexController,
    user_id: i64,
) -> Result<serde_json::Value, String> {
    let response = controller
        .command(&format!("/_servers {user_id}"))
        .map_err(|e| e.to_string())?;
    response
        .pointer("/result/userServers")
        .cloned()
        .ok_or_else(|| format!("user server response is invalid: {response}"))
}

fn save_user_servers(
    controller: &crate::ffi::SimplexController,
    user_id: i64,
    servers: &serde_json::Value,
) -> Result<(), String> {
    let response = controller
        .command(&format!("/_servers {user_id} {servers}"))
        .map_err(|e| e.to_string())?;
    ensure_ok(&response, "server configuration")
}

fn set_server_enabled(
    controller: &crate::ffi::SimplexController,
    user_id: i64,
    protocol: ServerProtocol,
    address: &str,
    enabled: bool,
) -> Result<(), String> {
    let mut groups = user_servers_json(controller, user_id)?;
    let mut found = false;
    for group in groups
        .as_array_mut()
        .ok_or("user servers are not an array")?
    {
        for server in group
            .get_mut(protocol.json_key())
            .and_then(serde_json::Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            if server.get("server").and_then(serde_json::Value::as_str) == Some(address) {
                server
                    .as_object_mut()
                    .ok_or("server entry is not an object")?
                    .insert("enabled".into(), enabled.into());
                found = true;
            }
        }
    }
    if !found {
        return Err("server is no longer present".into());
    }
    save_user_servers(controller, user_id, &groups)
}

fn add_server(
    controller: &crate::ffi::SimplexController,
    user_id: i64,
    protocol: ServerProtocol,
    address: &str,
) -> Result<(), String> {
    if !address.starts_with(match protocol {
        ServerProtocol::Smp => "smp://",
        ServerProtocol::Xftp => "xftp://",
    }) {
        return Err(format!(
            "expected an {}:// server address",
            protocol.label().to_ascii_lowercase()
        ));
    }
    let mut groups_json = user_servers_json(controller, user_id)?;
    let groups = groups_json
        .as_array_mut()
        .ok_or("user servers are not an array")?;
    if groups.iter().any(|group| {
        group
            .get(protocol.json_key())
            .and_then(serde_json::Value::as_array)
            .is_some_and(|servers| {
                servers.iter().any(|server| {
                    server.get("server").and_then(serde_json::Value::as_str) == Some(address)
                })
            })
    }) {
        return Err("server is already configured".into());
    }
    let custom_group = groups
        .iter_mut()
        .find(|group| group.get("operator").is_none_or(serde_json::Value::is_null))
        .ok_or("custom server group is missing")?;
    custom_group
        .get_mut(protocol.json_key())
        .and_then(serde_json::Value::as_array_mut)
        .ok_or("custom server list is missing")?
        .push(serde_json::json!({
            "server": address,
            "preset": false,
            "tested": null,
            "enabled": true,
            "roles": {"storage": null, "proxy": null, "names": null},
            "deleted": false
        }));
    save_user_servers(controller, user_id, &groups_json)
}

fn load_profiles(
    controller: &crate::ffi::SimplexController,
) -> Result<Vec<model::Profile>, String> {
    model::profiles(&controller.command("/users").map_err(|e| e.to_string())?)
}

fn load_chats(
    controller: &crate::ffi::SimplexController,
    user_id: i64,
) -> Result<Vec<model::ChatSummary>, String> {
    model::chats(
        &controller
            .command(&format!("/_get chats {user_id} pcc=on"))
            .map_err(|e| e.to_string())?,
    )
}

fn ensure_ok(value: &serde_json::Value, operation: &str) -> Result<(), String> {
    if value.get("error").is_some()
        || value
            .pointer("/result/type")
            .and_then(serde_json::Value::as_str)
            == Some("chatCmdError")
    {
        Err(format!("failed to update {operation}: {value}"))
    } else {
        Ok(())
    }
}

fn send_auto_delete(
    controller: &crate::ffi::SimplexController,
    sender: &Sender<SimplexEvent>,
    user_id: i64,
) -> Result<(), String> {
    let response = controller
        .command(&format!("/_ttl {user_id}"))
        .map_err(|e| e.to_string())?;
    let seconds = response
        .pointer("/result/chatItemTTL")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    sender
        .send(SimplexEvent::AutoDeleteLoaded(seconds))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_preferences_override_profile_defaults() {
        let defaults = model::ChatFeatures {
            full_deletion: false,
            reactions: true,
            voice_messages: true,
            files_and_media: true,
            ..model::ChatFeatures::default()
        };
        let direct = serde_json::json!({
            "fullDelete": {"allow": "yes"},
            "reactions": {"allow": "no"}
        });
        let features = features_from_preferences(Some(&direct), defaults.clone(), false);
        assert!(features.full_deletion);
        assert!(!features.reactions);
        assert!(features.voice_messages);

        let group = serde_json::json!({
            "voice": {"enable": "off"},
            "files": {"enable": "on"}
        });
        let features = features_from_preferences(Some(&group), defaults, true);
        assert!(!features.voice_messages);
        assert!(features.files_and_media);
    }

    #[test]
    fn connection_links_cannot_inject_another_command() {
        assert!(validate_connection_link("simplex:/invitation#example").is_ok());
        assert!(validate_connection_link("simplex:/invitation#example\n/_delete @1").is_err());
    }
}
