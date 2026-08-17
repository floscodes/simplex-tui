use serde_json::Value;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct User {
    pub id: i64,
    pub display_name: String,
    pub notifications: bool,
    pub active: bool,
}

pub type Profile = User;

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
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChatFeatures {
    pub disappearing_messages: bool,
    pub full_deletion: bool,
    pub reactions: bool,
    pub voice_messages: bool,
    pub files_and_media: bool,
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
    ServersLoaded(Vec<String>),
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
    let text = meta.get("itemText")?.as_str()?.to_owned();
    if text.is_empty() {
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
    })
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
}
