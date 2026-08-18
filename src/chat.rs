use serde_json::Value;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct User {
    pub id: i64,
    pub display_name: String,
    pub notifications: bool,
    pub active: bool,
}

pub type Profile = User;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServerProtocol {
    Smp,
    Xftp,
}

impl ServerProtocol {
    pub fn label(self) -> &'static str {
        match self {
            Self::Smp => "SMP",
            Self::Xftp => "XFTP",
        }
    }

    pub fn json_key(self) -> &'static str {
        match self {
            Self::Smp => "smpServers",
            Self::Xftp => "xftpServers",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerEntry {
    pub protocol: ServerProtocol,
    pub address: String,
    pub enabled: bool,
    pub preset: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChatSummary {
    pub chat_ref: ChatRef,
    pub display_name: String,
    pub unread_count: u64,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ChatRef(pub String);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Message {
    pub id: i64,
    pub text: String,
    pub timestamp: String,
    pub outgoing: bool,
    pub reactions: Vec<MessageReaction>,
    pub attachment: Option<Attachment>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttachmentKind {
    File,
    Audio,
    Image,
    Video,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attachment {
    pub id: i64,
    pub name: String,
    pub size: i64,
    pub kind: AttachmentKind,
    pub status: String,
    pub progress: Option<u8>,
    pub path: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MessageReaction {
    pub emoji: String,
    pub count: u64,
    pub user_reacted: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChatFeatures {
    pub disappearing_messages: bool,
    pub full_deletion: bool,
    pub reactions: bool,
    pub voice_messages: bool,
    pub files_and_media: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChatDeletionSettings {
    pub local_ttl: Option<i64>,
    pub disappearing_ttl: Option<i64>,
}

#[derive(Clone, Debug)]
pub enum SimplexEvent {
    Ready {
        user: User,
        chats: Vec<ChatSummary>,
    },
    ProfilesLoaded(Vec<Profile>),
    ProfileActivated {
        user: User,
        chats: Vec<ChatSummary>,
    },
    ProfileCreated {
        user: User,
        profiles: Vec<Profile>,
        chats: Vec<ChatSummary>,
    },
    ProfileDeleted {
        profiles: Vec<Profile>,
        active_user: Option<User>,
        chats: Vec<ChatSummary>,
    },
    ProfileDeleteFailed(String),
    SettingChanged(String),
    AutoDeleteLoaded(i64),
    AutoDeleteChanged(i64),
    AutoDeleteFailed(String),
    ChatDeletionLoaded {
        chat_ref: ChatRef,
        settings: ChatDeletionSettings,
    },
    ChatDeletionChanged {
        chat_ref: ChatRef,
        settings: ChatDeletionSettings,
    },
    ChatDeletionFailed(String),
    FileDownloadStarted {
        file_id: i64,
        path: String,
    },
    FileDownloadFailed {
        file_id: i64,
        error: String,
    },
    FileDownloadCancelled {
        file_id: i64,
    },
    FileUpdated {
        chat_ref: ChatRef,
        message: Message,
    },
    ServersLoaded(Vec<ServerEntry>),
    ServersUpdateFailed(String),
    ChatFeaturesLoaded(ChatFeatures),
    InvitationCreated(String),
    InvitationFailed(String),
    ChatLoaded {
        chat_ref: ChatRef,
        messages: Vec<Message>,
    },
    ChatMarkedRead(ChatRef),
    ContactConnected {
        chats: Vec<ChatSummary>,
        chat_ref: ChatRef,
    },
    MessageReceived {
        chat_ref: ChatRef,
        message: Message,
    },
    ChatLoadFailed {
        chat_ref: ChatRef,
        error: String,
    },
    MessageSent {
        chat_ref: ChatRef,
        text: String,
    },
    MessageSendFailed {
        chat_ref: ChatRef,
        error: String,
    },
    ReactionChanged {
        chat_ref: ChatRef,
        item_id: i64,
        emoji: String,
        added: bool,
        user_reacted: bool,
    },
    NoActiveUser,
    Failed(String),
}

pub fn invitation_link(value: &Value) -> Result<String, String> {
    let result = value
        .get("result")
        .ok_or_else(|| response_error(value, "invitation"))?;
    if result.get("type").and_then(Value::as_str) != Some("invitation") {
        return Err(response_error(value, "invitation"));
    }
    let link = result
        .pointer("/connLinkInvitation/connShortLink")
        .and_then(Value::as_str)
        .filter(|link| !link.is_empty())
        .or_else(|| {
            result
                .pointer("/connLinkInvitation/connFullLink")
                .and_then(Value::as_str)
        })
        .ok_or("invitation response has no connection link")?;
    Ok(link.to_owned())
}

pub fn smp_servers(value: &Value) -> Result<Vec<String>, String> {
    let result = value
        .get("result")
        .ok_or_else(|| response_error(value, "SMP servers"))?;
    if result.get("type").and_then(Value::as_str) != Some("userServers") {
        return Err(response_error(value, "SMP servers"));
    }
    let mut servers = Vec::new();
    collect_smp_servers(result, &mut servers);
    servers.sort();
    servers.dedup();
    Ok(servers)
}

pub fn server_entries(value: &Value) -> Result<Vec<ServerEntry>, String> {
    let groups = value
        .pointer("/result/userServers")
        .and_then(Value::as_array)
        .ok_or_else(|| response_error(value, "user servers"))?;
    let mut entries = Vec::new();
    for group in groups {
        for protocol in [ServerProtocol::Smp, ServerProtocol::Xftp] {
            for server in group
                .get(protocol.json_key())
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(address) = server.get("server").and_then(Value::as_str) else {
                    continue;
                };
                if server
                    .get("deleted")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    continue;
                }
                entries.push(ServerEntry {
                    protocol,
                    address: address.to_owned(),
                    enabled: server
                        .get("enabled")
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                    preset: server
                        .get("preset")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                });
            }
        }
    }
    Ok(entries)
}

pub fn profile_and_features(value: &Value) -> Result<(Value, ChatFeatures), String> {
    let result = value
        .get("result")
        .ok_or_else(|| response_error(value, "user profile"))?;
    if result.get("type").and_then(Value::as_str) != Some("userProfile") {
        return Err(response_error(value, "user profile"));
    }
    let profile = result
        .get("profile")
        .cloned()
        .ok_or("userProfile response has no profile")?;
    let preferences = profile.get("preferences");
    let enabled = |name: &str, default: bool| {
        preferences
            .and_then(|prefs| prefs.get(name))
            .and_then(|pref| pref.get("allow"))
            .and_then(Value::as_str)
            .map(|allow| allow != "no")
            .unwrap_or(default)
    };
    let features = ChatFeatures {
        disappearing_messages: enabled("timedMessages", false),
        full_deletion: enabled("fullDelete", false),
        reactions: enabled("reactions", true),
        voice_messages: enabled("voice", true),
        files_and_media: enabled("files", true),
    };
    Ok((profile, features))
}

pub fn connected_contact(value: &Value) -> Option<(i64, ChatRef)> {
    let result = value.get("result")?;
    if result.get("type")?.as_str()? != "contactConnected" {
        return None;
    }
    let user_id = result.pointer("/user/userId")?.as_i64()?;
    let contact_id = result.pointer("/contact/contactId")?.as_i64()?;
    Some((user_id, ChatRef(format!("@{contact_id}"))))
}

pub fn reaction_change(value: &Value) -> Option<(ChatRef, i64, String, bool, bool)> {
    let result = value.get("result")?;
    if result.get("type")?.as_str()? != "chatItemReaction" {
        return None;
    }
    let reaction = result.get("reaction")?;
    let chat_ref = chat_ref(reaction.get("chatInfo")?).ok()?;
    let chat_reaction = reaction.get("chatReaction")?;
    let item_id = chat_reaction.pointer("/chatItem/meta/itemId")?.as_i64()?;
    let emoji = chat_reaction
        .pointer("/reaction/emoji")?
        .as_str()?
        .to_owned();
    let direction = chat_reaction.pointer("/chatDir/type")?.as_str()?;
    Some((
        chat_ref,
        item_id,
        emoji,
        result.get("added")?.as_bool()?,
        direction.to_ascii_lowercase().contains("snd"),
    ))
}

fn collect_smp_servers(value: &Value, servers: &mut Vec<String>) {
    match value {
        Value::String(server) if server.starts_with("smp://") => servers.push(server.clone()),
        Value::Array(values) => {
            for value in values {
                collect_smp_servers(value, servers);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                collect_smp_servers(value, servers);
            }
        }
        _ => {}
    }
}

pub fn active_user(value: &Value) -> Result<Option<User>, String> {
    if value
        .pointer("/error/errorType/type")
        .and_then(Value::as_str)
        == Some("noActiveUser")
    {
        return Ok(None);
    }
    let result = value
        .get("result")
        .ok_or_else(|| response_error(value, "active user"))?;
    if result.get("type").and_then(Value::as_str) != Some("activeUser") {
        return Err(response_error(value, "active user"));
    }
    let user = result
        .get("user")
        .ok_or("activeUser response has no user")?;
    Ok(Some(User {
        id: user
            .get("userId")
            .and_then(Value::as_i64)
            .ok_or("activeUser response has no numeric userId")?,
        display_name: user
            .get("localDisplayName")
            .and_then(Value::as_str)
            .unwrap_or("SimpleX user")
            .to_owned(),
        notifications: user
            .get("showNtfs")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        active: user
            .get("activeUser")
            .and_then(Value::as_bool)
            .unwrap_or(true),
    }))
}

pub fn profiles(value: &Value) -> Result<Vec<Profile>, String> {
    let result = value
        .get("result")
        .ok_or_else(|| response_error(value, "profiles"))?;
    if result.get("type").and_then(Value::as_str) != Some("usersList") {
        return Err(response_error(value, "profiles"));
    }
    result
        .get("users")
        .and_then(Value::as_array)
        .ok_or("usersList response has no users array")?
        .iter()
        .map(|info| {
            let user = info.get("user").ok_or("profile has no user")?;
            Ok(Profile {
                id: user
                    .get("userId")
                    .and_then(Value::as_i64)
                    .ok_or("profile has no numeric userId")?,
                display_name: user
                    .get("localDisplayName")
                    .and_then(Value::as_str)
                    .unwrap_or("SimpleX user")
                    .to_owned(),
                notifications: user
                    .get("showNtfs")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                active: user
                    .get("activeUser")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect()
}

pub fn chats(value: &Value) -> Result<Vec<ChatSummary>, String> {
    let result = value
        .get("result")
        .ok_or_else(|| response_error(value, "chat list"))?;
    if !matches!(
        result.get("type").and_then(Value::as_str),
        Some("apiChats" | "chats")
    ) {
        return Err(response_error(value, "chat list"));
    }
    result
        .get("chats")
        .and_then(Value::as_array)
        .ok_or("chat-list response has no chats array")?
        .iter()
        .filter(|chat| {
            let Some(info) = chat.get("chatInfo") else {
                return false;
            };
            if info.get("type").and_then(Value::as_str) == Some("local") {
                return false;
            }
            info.pointer("/contact/localDisplayName")
                .and_then(Value::as_str)
                != Some("Ask SimpleX Team")
        })
        .map(|chat| {
            let info = chat.get("chatInfo").ok_or("chat has no chatInfo")?;
            let chat_ref = chat_ref(info)?;
            let display_name = [
                "contact",
                "groupInfo",
                "noteFolder",
                "contactRequest",
                "contactConnection",
            ]
            .iter()
            .find_map(|key| info.get(key))
            .and_then(|info| info.get("localDisplayName"))
            .and_then(Value::as_str)
            .or_else(|| info.get("localDisplayName").and_then(Value::as_str))
            .unwrap_or("Unknown chat")
            .to_owned();
            let unread_count = chat
                .pointer("/chatStats/unreadCount")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            Ok(ChatSummary {
                chat_ref,
                display_name,
                unread_count,
            })
        })
        .collect()
}

pub fn chat_messages(value: &Value) -> Result<(ChatRef, Vec<Message>), String> {
    let result = value
        .get("result")
        .ok_or_else(|| response_error(value, "chat"))?;
    if result.get("type").and_then(Value::as_str) != Some("apiChat") {
        return Err(response_error(value, "chat"));
    }
    let chat = result.get("chat").ok_or("apiChat response has no chat")?;
    let chat_ref = chat_ref(chat.get("chatInfo").ok_or("chat has no chatInfo")?)?;
    let messages = chat
        .get("chatItems")
        .and_then(Value::as_array)
        .ok_or("chat has no chatItems array")?
        .iter()
        .filter_map(parse_message)
        .collect();
    Ok((chat_ref, messages))
}

pub fn new_messages(value: &Value) -> Vec<(ChatRef, Message)> {
    let Some(result) = value.get("result") else {
        return Vec::new();
    };
    if result.get("type").and_then(Value::as_str) != Some("newChatItems") {
        return Vec::new();
    }
    result
        .get("chatItems")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let chat_ref = chat_ref(item.get("chatInfo")?).ok()?;
            let message = parse_message(item.get("chatItem")?)?;
            Some((chat_ref, message))
        })
        .collect()
}

fn chat_ref(info: &Value) -> Result<ChatRef, String> {
    let (prefix, object, id_key) = match info.get("type").and_then(Value::as_str) {
        Some("direct") => ("@", "contact", "contactId"),
        Some("group") => ("#", "groupInfo", "groupId"),
        Some("local") => ("*", "noteFolder", "noteFolderId"),
        Some("contactRequest") => ("<@", "contactRequest", "contactRequestId"),
        Some("contactConnection") => (":", "contactConnection", "pccConnId"),
        _ => return Err("unsupported chat type".into()),
    };
    let id = info
        .get(object)
        .and_then(|v| v.get(id_key))
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("chatInfo has no numeric {id_key}"))?;
    Ok(ChatRef(format!("{prefix}{id}")))
}

fn parse_message(item: &Value) -> Option<Message> {
    if !matches!(
        item.pointer("/content/type").and_then(Value::as_str),
        Some("sndMsgContent" | "rcvMsgContent")
    ) {
        return None;
    }
    let meta = item.get("meta")?;
    let text = meta
        .get("itemText")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let message_kind = item
        .pointer("/content/msgContent/type")
        .and_then(Value::as_str);
    let attachment = item
        .get("file")
        .and_then(|file| parse_attachment(file, message_kind));
    if text.is_empty() && attachment.is_none() {
        return None;
    }
    let direction = item
        .pointer("/chatDir/type")
        .and_then(Value::as_str)
        .unwrap_or("");
    Some(Message {
        id: meta.get("itemId")?.as_i64()?,
        text,
        timestamp: meta
            .get("itemTs")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        outgoing: direction.to_ascii_lowercase().contains("snd"),
        reactions: item
            .get("reactions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|reaction| {
                Some(MessageReaction {
                    emoji: reaction.pointer("/reaction/emoji")?.as_str()?.to_owned(),
                    count: reaction.get("totalReacted")?.as_u64()?,
                    user_reacted: reaction
                        .get("userReacted")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                })
            })
            .collect(),
        attachment,
    })
}

fn parse_attachment(file: &Value, content_type: Option<&str>) -> Option<Attachment> {
    let name = file.get("fileName")?.as_str()?.to_owned();
    let kind = match content_type {
        Some("image") => AttachmentKind::Image,
        Some("video") => AttachmentKind::Video,
        Some("voice") => AttachmentKind::Audio,
        _ => attachment_kind_from_name(&name),
    };
    Some(Attachment {
        id: file.get("fileId")?.as_i64()?,
        name,
        size: file.get("fileSize").and_then(Value::as_i64).unwrap_or(0),
        kind,
        status: file
            .pointer("/fileStatus/type")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned(),
        progress: file
            .pointer("/fileStatus/rcvProgress")
            .and_then(Value::as_i64)
            .zip(file.pointer("/fileStatus/rcvTotal").and_then(Value::as_i64))
            .and_then(|(progress, total)| {
                (total > 0).then(|| {
                    u8::try_from(
                        progress
                            .saturating_mul(100)
                            .saturating_div(total)
                            .clamp(0, 100),
                    )
                    .unwrap_or(100)
                })
            }),
        path: file
            .pointer("/fileSource/filePath")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

fn attachment_kind_from_name(name: &str) -> AttachmentKind {
    let extension = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match extension.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "svg" | "heic" => AttachmentKind::Image,
        "mp3" | "m4a" | "aac" | "wav" | "ogg" | "opus" | "flac" => AttachmentKind::Audio,
        "mp4" | "m4v" | "mkv" | "mov" | "webm" | "avi" => AttachmentKind::Video,
        _ => AttachmentKind::File,
    }
}

pub fn file_update(value: &Value) -> Option<(ChatRef, Message)> {
    let result = value.get("result")?;
    let item = result.get("chatItem").or_else(|| result.get("chatItem_"))?;
    let mut message = parse_message(item.get("chatItem")?)?;
    if let Some(attachment) = &mut message.attachment
        && let Some((received, total)) = result
            .get("receivedSize")
            .and_then(Value::as_i64)
            .zip(result.get("totalSize").and_then(Value::as_i64))
        && total > 0
    {
        attachment.progress = Some(
            u8::try_from(
                received
                    .saturating_mul(100)
                    .saturating_div(total)
                    .clamp(0, 100),
            )
            .unwrap_or(100),
        );
    }
    Some((chat_ref(item.get("chatInfo")?).ok()?, message))
}

fn response_error(value: &Value, expected: &str) -> String {
    let detail = value
        .pointer("/error/errorType/type")
        .or_else(|| value.pointer("/error/type"))
        .or_else(|| value.pointer("/result/type"))
        .and_then(Value::as_str)
        .unwrap_or("unknown response");
    format!("expected {expected}, received {detail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_active_user_and_no_user() {
        let user = active_user(&json!({"result": {"type": "activeUser", "user": {
            "userId": 7, "localDisplayName": "alice"
        }}}))
        .unwrap()
        .unwrap();
        assert_eq!(
            user,
            User {
                id: 7,
                display_name: "alice".into(),
                notifications: true,
                active: true,
            }
        );
        assert_eq!(
            active_user(&json!({"error": {"type": "error", "errorType": {
                "type": "noActiveUser"
            }}}))
            .unwrap(),
            None
        );
    }

    #[test]
    fn parses_chat_summaries() {
        let parsed = chats(&json!({"result": {"type": "apiChats", "chats": [{
            "chatInfo": {"type": "direct", "contact": {"contactId": 9, "localDisplayName": "bob"}},
            "chatItems": [], "chatStats": {"unreadCount": 3}
        }]}}))
        .unwrap();
        assert_eq!(
            parsed,
            vec![ChatSummary {
                chat_ref: ChatRef("@9".into()),
                display_name: "bob".into(),
                unread_count: 3
            }]
        );
    }

    #[test]
    fn parses_chat_history_and_live_item() {
        let info =
            json!({"type": "direct", "contact": {"contactId": 9, "localDisplayName": "bob"}});
        let item = json!({"chatDir": {"type": "directSnd"},
            "content": {"type": "sndMsgContent", "msgContent": {"type": "text", "text": "hello"}}, "meta": {
            "itemId": 12, "itemText": "hello", "itemTs": "2026-08-16T10:00:00Z"
        }});
        let (chat_ref, messages) = chat_messages(&json!({"result": {"type": "apiChat", "chat": {
            "chatInfo": info, "chatItems": [item], "chatStats": {}
        }}}))
        .unwrap();
        assert_eq!(chat_ref, ChatRef("@9".into()));
        assert!(messages[0].outgoing);
        assert_eq!(messages[0].text, "hello");
    }

    #[test]
    fn protocol_feature_items_are_not_user_messages() {
        let item = json!({
            "chatDir": {"type": "directSnd"},
            "content": {"type": "sndChatFeature", "feature": "calls"},
            "meta": {"itemId": 13, "itemText": "Audio/video calls: off"}
        });
        assert_eq!(parse_message(&item), None);
    }

    #[test]
    fn parses_profile_chat_features() {
        let (_, features) = profile_and_features(&json!({"result": {
            "type": "userProfile", "profile": {
                "displayName": "alice", "fullName": "", "preferences": {
                    "timedMessages": {"allow": "no"},
                    "fullDelete": {"allow": "yes"},
                    "reactions": {"allow": "yes"},
                    "voice": {"allow": "no"},
                    "files": {"allow": "yes"},
                    "calls": {"allow": "no"}
                }
            }
        }}))
        .unwrap();
        assert!(!features.disappearing_messages);
        assert!(features.full_deletion);
        assert!(features.reactions);
        assert!(!features.voice_messages);
        assert!(features.files_and_media);
    }

    #[test]
    fn prefers_the_short_invitation_link() {
        let link = invitation_link(&json!({"result": {
            "type": "invitation",
            "connLinkInvitation": {
                "connFullLink": "simplex:/contact#full",
                "connShortLink": "https://simplex.chat/contact#short"
            }
        }}))
        .unwrap();
        assert_eq!(link, "https://simplex.chat/contact#short");
    }

    #[test]
    fn extracts_and_deduplicates_profile_smp_servers() {
        let servers = smp_servers(&json!({"result": {
            "type": "userServers",
            "userServers": [{"smpServers": [
                {"server": "smp://fingerprint@smp11.simplex.im,onion"},
                {"server": "smp://fingerprint@smp11.simplex.im,onion"}
            ]}]
        }}))
        .unwrap();
        assert_eq!(
            servers,
            vec!["smp://fingerprint@smp11.simplex.im,onion".to_owned()]
        );
    }

    #[test]
    fn parses_smp_and_xftp_server_configuration() {
        let servers = server_entries(&json!({"result": {
            "type": "userServers",
            "userServers": [{
                "operator": {"tradeName": "SimpleX Chat"},
                "smpServers": [{
                    "server": "smp://key@smp.example",
                    "enabled": true,
                    "preset": true,
                    "deleted": false
                }],
                "xftpServers": [{
                    "server": "xftp://key@xftp.example",
                    "enabled": false,
                    "preset": true,
                    "deleted": false
                }]
            }]
        }}))
        .unwrap();
        assert_eq!(
            servers,
            vec![
                ServerEntry {
                    protocol: ServerProtocol::Smp,
                    address: "smp://key@smp.example".into(),
                    enabled: true,
                    preset: true,
                },
                ServerEntry {
                    protocol: ServerProtocol::Xftp,
                    address: "xftp://key@xftp.example".into(),
                    enabled: false,
                    preset: true,
                },
            ]
        );
    }

    #[test]
    fn recognizes_a_connected_invitation_contact() {
        let connected = connected_contact(&json!({"result": {
            "type": "contactConnected",
            "user": {"userId": 4},
            "contact": {"contactId": 19, "localDisplayName": "bob"}
        }}));
        assert_eq!(connected, Some((4, ChatRef("@19".into()))));
        assert_eq!(
            connected_contact(&json!({"result": {"type": "cmdOk"}})),
            None
        );
    }

    #[test]
    fn parses_message_reactions_and_live_reaction_changes() {
        let item = json!({
            "chatDir": {"type": "directRcv"},
            "content": {"type": "rcvMsgContent", "msgContent": {"type": "text", "text": "hi"}},
            "meta": {"itemId": 22, "itemText": "hi"},
            "reactions": [{
                "reaction": {"type": "emoji", "emoji": "👍"},
                "userReacted": true,
                "totalReacted": 3
            }]
        });
        let message = parse_message(&item).unwrap();
        assert_eq!(
            message.reactions,
            vec![MessageReaction {
                emoji: "👍".into(),
                count: 3,
                user_reacted: true,
            }]
        );

        let update = reaction_change(&json!({"result": {
            "type": "chatItemReaction",
            "added": true,
            "reaction": {
                "chatInfo": {"type": "direct", "contact": {"contactId": 7}},
                "chatReaction": {
                    "chatDir": {"type": "directRcv"},
                    "chatItem": {"meta": {"itemId": 22}},
                    "reaction": {"type": "emoji", "emoji": "😂"}
                }
            }
        }}));
        assert_eq!(
            update,
            Some((ChatRef("@7".into()), 22, "😂".into(), true, false))
        );
    }

    #[test]
    fn parses_file_only_messages_and_file_updates() {
        let chat_item = json!({
            "chatDir": {"type": "directRcv"},
            "content": {
                "type": "rcvMsgContent",
                "msgContent": {"type": "image", "text": "", "image": "preview"}
            },
            "meta": {"itemId": 33, "itemText": ""},
            "reactions": [],
            "file": {
                "fileId": 44,
                "fileName": "photo.png",
                "fileSize": 1234,
                "fileStatus": {"type": "rcvInvitation"},
                "fileProtocol": "xftp"
            }
        });
        let message = parse_message(&chat_item).unwrap();
        let attachment = message.attachment.unwrap();
        assert_eq!(attachment.kind, AttachmentKind::Image);
        assert_eq!(attachment.name, "photo.png");
        assert_eq!(attachment.status, "rcvInvitation");

        let update = json!({"result": {
            "type": "rcvFileProgressXFTP",
            "receivedSize": 25,
            "totalSize": 100,
            "chatItem_": {
                "chatInfo": {"type": "direct", "contact": {"contactId": 7}},
                "chatItem": chat_item
            }
        }});
        let (chat_ref, updated) = file_update(&update).unwrap();
        assert_eq!(chat_ref, ChatRef("@7".into()));
        assert_eq!(updated.id, 33);
        assert_eq!(updated.attachment.unwrap().progress, Some(25));
    }
}
