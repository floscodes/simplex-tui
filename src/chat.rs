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
    SettingChanged(String),
    AutoDeleteLoaded(i64),
    ChatLoaded {
        chat_ref: ChatRef,
        messages: Vec<Message>,
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
        let item = json!({"chatDir": {"type": "directSnd"}, "meta": {
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
}
