use std::{
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::Duration,
};

use crate::{
    chat::{self, ChatRef, SimplexEvent},
    simplex::{SimplexApi, SimplexPaths},
};

#[derive(Debug)]
pub enum SimplexCommand {
    LoadChat(ChatRef),
    MarkChatRead(ChatRef),
    SendMessage {
        chat_ref: ChatRef,
        text: String,
    },
    ActivateProfile(i64),
    CreateProfile(String),
    SetNotifications {
        user_id: i64,
        enabled: bool,
    },
    SetAutoDelete {
        user_id: i64,
        seconds: i64,
    },
    CreateInvitation {
        user_id: i64,
    },
    SetChatFeature {
        user_id: i64,
        feature: ChatFeature,
        enabled: bool,
    },
}

#[derive(Clone, Copy, Debug)]
pub enum ChatFeature {
    DisappearingMessages,
    FullDeletion,
    Reactions,
    VoiceMessages,
    FilesAndMedia,
}

pub fn spawn(api: Arc<SimplexApi>, sender: Sender<SimplexEvent>) -> Sender<SimplexCommand> {
    let (command_sender, commands) = mpsc::channel();
    thread::Builder::new()
        .name("simplex-core".into())
        .spawn(move || {
            if let Err(error) = run(&api, &sender, &commands) {
                let _ = sender.send(SimplexEvent::Failed(error));
            }
        })
        .expect("failed to spawn SimpleX worker");
    command_sender
}

fn run(
    api: &Arc<SimplexApi>,
    sender: &Sender<SimplexEvent>,
    commands: &Receiver<SimplexCommand>,
) -> Result<(), String> {
    let paths = SimplexPaths::discover().map_err(|e| e.to_string())?;
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

    let Some(user) = chat::active_user(&controller.command("/u").map_err(|e| e.to_string())?)?
    else {
        sender
            .send(SimplexEvent::NoActiveUser)
            .map_err(|e| e.to_string())?;
        return command_loop(controller, sender, commands);
    };
    controller.command("/_start").map_err(|e| e.to_string())?;
    send_auto_delete(&controller, sender, user.id)?;
    send_servers(&controller, sender, user.id)?;
    load_chat_features(&controller, sender, user.id, true)?;
    let chats = load_chats(&controller, user.id)?;
    sender
        .send(SimplexEvent::Ready { user, chats })
        .map_err(|e| e.to_string())?;

    command_loop(controller, sender, commands)
}

fn command_loop(
    controller: crate::simplex::SimplexController,
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
                        .and_then(|v| chat::chat_messages(&v))
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
                            for (item_ref, message) in chat::new_messages(&response) {
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
                SimplexCommand::ActivateProfile(user_id) => {
                    let result = (|| {
                        let response = controller
                            .command(&format!("/_user {user_id}"))
                            .map_err(|e| e.to_string())?;
                        let user = chat::active_user(&response)?
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
                        let user = chat::active_user(&response)?
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
                SimplexCommand::SetNotifications { user_id, enabled } => {
                    let action = if enabled { "unmute" } else { "mute" };
                    let response = controller
                        .command(&format!("/_{action} user {user_id}"))
                        .map_err(|e| e.to_string())?;
                    ensure_ok(&response, "notifications")?;
                    sender
                        .send(SimplexEvent::SettingChanged(format!(
                            "Notifications {}",
                            if enabled { "enabled" } else { "disabled" }
                        )))
                        .map_err(|e| e.to_string())?;
                }
                SimplexCommand::SetAutoDelete { user_id, seconds } => {
                    let response = controller
                        .command(&format!("/_ttl {user_id} {seconds}"))
                        .map_err(|e| e.to_string())?;
                    ensure_ok(&response, "automatic deletion")?;
                    sender
                        .send(SimplexEvent::SettingChanged(
                            "Automatic deletion updated".into(),
                        ))
                        .map_err(|e| e.to_string())?;
                }
                SimplexCommand::CreateInvitation { user_id } => {
                    let event = controller
                        .command(&format!("/_connect {user_id} incognito=off"))
                        .map_err(|e| e.to_string())
                        .and_then(|value| chat::invitation_link(&value))
                        .map(SimplexEvent::InvitationCreated)
                        .unwrap_or_else(SimplexEvent::InvitationFailed);
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
            }
        }
        if let Some(value) = controller
            .recv(Duration::from_millis(200))
            .map_err(|e| e.to_string())?
        {
            for (chat_ref, message) in chat::new_messages(&value) {
                sender
                    .send(SimplexEvent::MessageReceived { chat_ref, message })
                    .map_err(|e| e.to_string())?;
            }
        }
    }
}

fn mark_chat_read(
    controller: &crate::simplex::SimplexController,
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
    controller: &crate::simplex::SimplexController,
    sender: &Sender<SimplexEvent>,
    user_id: i64,
    enforce_calls_disabled: bool,
) -> Result<(), String> {
    let response = controller.command("/profile").map_err(|e| e.to_string())?;
    let (mut profile, features) = chat::profile_and_features(&response)?;
    if enforce_calls_disabled && preference_allow(&profile, "calls") != Some("no") {
        set_profile_preference(&mut profile, "calls", false)?;
        update_profile(controller, user_id, &profile)?;
    }
    sender
        .send(SimplexEvent::ChatFeaturesLoaded(features))
        .map_err(|e| e.to_string())
}

fn update_chat_feature(
    controller: &crate::simplex::SimplexController,
    user_id: i64,
    feature: ChatFeature,
    enabled: bool,
) -> Result<chat::ChatFeatures, String> {
    let response = controller.command("/profile").map_err(|e| e.to_string())?;
    let (mut profile, _) = chat::profile_and_features(&response)?;
    let name = match feature {
        ChatFeature::DisappearingMessages => "timedMessages",
        ChatFeature::FullDeletion => "fullDelete",
        ChatFeature::Reactions => "reactions",
        ChatFeature::VoiceMessages => "voice",
        ChatFeature::FilesAndMedia => "files",
    };
    set_profile_preference(&mut profile, name, enabled)?;
    set_profile_preference(&mut profile, "calls", false)?;
    update_profile(controller, user_id, &profile)?;
    let synthetic = serde_json::json!({"result": {"type": "userProfile", "profile": profile}});
    chat::profile_and_features(&synthetic).map(|(_, features)| features)
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
    controller: &crate::simplex::SimplexController,
    user_id: i64,
    profile: &serde_json::Value,
) -> Result<(), String> {
    let response = controller
        .command(&format!("/_profile {user_id} {profile}"))
        .map_err(|e| e.to_string())?;
    ensure_ok(&response, "chat preferences")
}

fn send_servers(
    controller: &crate::simplex::SimplexController,
    sender: &Sender<SimplexEvent>,
    user_id: i64,
) -> Result<(), String> {
    let response = controller
        .command(&format!("/_servers {user_id}"))
        .map_err(|e| e.to_string())?;
    let servers = chat::smp_servers(&response)?;
    sender
        .send(SimplexEvent::ServersLoaded(servers))
        .map_err(|e| e.to_string())
}

fn load_profiles(
    controller: &crate::simplex::SimplexController,
) -> Result<Vec<chat::Profile>, String> {
    chat::profiles(&controller.command("/users").map_err(|e| e.to_string())?)
}

fn load_chats(
    controller: &crate::simplex::SimplexController,
    user_id: i64,
) -> Result<Vec<chat::ChatSummary>, String> {
    chat::chats(
        &controller
            .command(&format!("/_get chats {user_id} pcc=on"))
            .map_err(|e| e.to_string())?,
    )
}

fn ensure_ok(value: &serde_json::Value, operation: &str) -> Result<(), String> {
    if value.get("error").is_some() {
        Err(format!("failed to update {operation}: {value}"))
    } else {
        Ok(())
    }
}

fn send_auto_delete(
    controller: &crate::simplex::SimplexController,
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
