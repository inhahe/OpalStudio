//! Saving the preferences, tool settings and presets between sessions.

use crate::settings::Value;
use crate::state::{AppState, Background, NewImage};
use crate::tools::ToolId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

type Map = BTreeMap<String, Value>;

/// What is written to `settings.json`. Only values changed from their defaults are stored,
/// so a default changed in a later version reaches everyone who never changed it.
#[derive(Serialize, Deserialize, Default)]
struct Saved {
    #[serde(default)]
    prefs: Map,
    /// Per tool identifier.
    #[serde(default)]
    tools: BTreeMap<String, Map>,
    /// Per tool identifier, per preset name.
    #[serde(default)]
    presets: BTreeMap<String, BTreeMap<String, Map>>,
    #[serde(default)]
    last_new: Option<(u32, u32, bool)>,
}

/// Where the settings are kept: `settings.json` in the per-user configuration folder.
pub fn settings_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "Opal Studio").map(|d| d.config_dir().join("settings.json"))
}

fn tool_by_id(id: &str) -> Option<ToolId> {
    ToolId::ALL.into_iter().find(|t| t.info().id == id)
}

/// Loads the saved settings into `st`. A missing file is not an error; an unreadable one is
/// reported, and the defaults are used.
pub fn load(st: &mut AppState) {
    let Some(path) = settings_path() else { return };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            st.message(crate::state::MessageKind::Warning, "Settings not loaded", format!("{}: {e}", path.display()));
            return;
        }
    };
    let saved: Saved = match serde_json::from_str(&text) {
        Ok(s) => s,
        Err(e) => {
            st.message(
                crate::state::MessageKind::Warning,
                "Settings not loaded",
                format!("{} could not be read ({e}); the defaults are used, and the file will be replaced when settings change.", path.display()),
            );
            return;
        }
    };
    st.prefs.load_map(&saved.prefs);
    for (id, map) in &saved.tools {
        if let Some(t) = tool_by_id(id)
            && let Some(s) = st.tool_settings.get_mut(&t)
        {
            s.load_map(map);
        }
    }
    for (id, presets) in saved.presets {
        if let Some(t) = tool_by_id(&id) {
            st.presets.insert(t, presets);
        }
    }
    if let Some((width, height, white)) = saved.last_new {
        let background = if white { Background::White } else { Background::Transparent };
        st.last_new = NewImage { width, height, background };
    }
}

/// Writes the settings, replacing the file only once the new one is complete.
pub fn save(st: &AppState) -> std::io::Result<()> {
    let Some(path) = settings_path() else { return Ok(()) };
    let saved = Saved {
        prefs: st.prefs.to_map(),
        tools: st
            .tool_settings
            .iter()
            .map(|(t, s)| (t.info().id.to_owned(), s.to_map()))
            .filter(|(_, m)| !m.is_empty())
            .collect(),
        presets: st.presets.iter().map(|(t, p)| (t.info().id.to_owned(), p.clone())).collect(),
        last_new: Some((st.last_new.width, st.last_new.height, st.last_new.background == Background::White)),
    };
    let json = serde_json::to_string_pretty(&saved).map_err(std::io::Error::other)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_settings_round_trip_through_json() {
        let mut st = AppState::new();
        st.prefs.set("ui.theme", Value::Choice("light".into()));
        st.tool_settings.get_mut(&ToolId::Zoom).unwrap().set("zoom.drag", Value::Bool(false));
        let saved = Saved {
            prefs: st.prefs.to_map(),
            tools: st.tool_settings.iter().map(|(t, s)| (t.info().id.to_owned(), s.to_map())).collect(),
            presets: BTreeMap::new(),
            last_new: None,
        };
        let json = serde_json::to_string(&saved).unwrap();
        let back: Saved = serde_json::from_str(&json).unwrap();
        let mut fresh = AppState::new();
        fresh.prefs.load_map(&back.prefs);
        assert_eq!(fresh.prefs.choice("ui.theme"), "light");
        assert_eq!(back.tools["zoom"].get("zoom.drag"), Some(&Value::Bool(false)));
        // An empty or partial file still loads.
        let partial: Saved = serde_json::from_str("{}").unwrap();
        assert!(partial.prefs.is_empty());
    }
}
