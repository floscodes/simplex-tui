use std::{fs, io, path::Path};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum Theme {
    #[default]
    Terminal,
    Dark,
    Light,
}

impl Theme {
    pub fn next(self) -> Self {
        match self {
            Self::Terminal => Self::Dark,
            Self::Dark => Self::Light,
            Self::Light => Self::Terminal,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Terminal => "Terminal default",
            Self::Dark => "Dark",
            Self::Light => "Light",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct Preferences {
    pub theme: Theme,
    pub compact_messages: bool,
    pub message_preview: bool,
}

impl Preferences {
    pub fn load(root: &Path) -> Self {
        fs::read(root.join("state.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, root: &Path) -> io::Result<()> {
        fs::create_dir_all(root)?;
        let target = root.join("state.json");
        let temporary = root.join("state.json.tmp");
        fs::write(&temporary, serde_json::to_vec_pretty(self)?)?;
        fs::rename(temporary, target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_round_trip() {
        let root = tempfile::tempdir().unwrap();
        let preferences = Preferences {
            theme: Theme::Dark,
            compact_messages: true,
            message_preview: false,
        };
        preferences.save(root.path()).unwrap();
        assert_eq!(Preferences::load(root.path()), preferences);
    }
}
