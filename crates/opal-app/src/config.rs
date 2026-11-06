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
    /// The scripts run lately, the last first.
    #[serde(default)]
    recent_scripts: Vec<PathBuf>,
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
    if let Err(e) = apply_json(st, &text) {
        // Keep the unreadable file, so nothing in it is lost when settings are next saved.
        let kept = path.with_extension("json.unreadable");
        let note = match std::fs::rename(&path, &kept) {
            Ok(()) => format!("It was kept as {}.", kept.display()),
            Err(_) => String::new(),
        };
        st.message(
            crate::state::MessageKind::Warning,
            "Settings not loaded",
            format!("{} could not be read ({e}); the defaults are used. {note}", path.display()),
        );
    }
}

/// Takes the settings stored in `text` into `st`. Entries that cannot be read (from a newer
/// version, say) are skipped one by one; only a file that is not JSON at all is an error.
fn apply_json(st: &mut AppState, text: &str) -> Result<(), String> {
    let root: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let obj = root.as_object().ok_or("the file does not hold an object")?;
    let map_of = |v: Option<&serde_json::Value>| -> Map {
        v.and_then(serde_json::Value::as_object)
            .map(|m| {
                m.iter().filter_map(|(k, v)| serde_json::from_value(v.clone()).ok().map(|v| (k.clone(), v))).collect()
            })
            .unwrap_or_default()
    };
    st.prefs.load_map(&map_of(obj.get("prefs")));
    for (id, map) in obj.get("tools").and_then(serde_json::Value::as_object).into_iter().flatten() {
        if let Some(t) = tool_by_id(id)
            && let Some(s) = st.tool_settings.get_mut(&t)
        {
            s.load_map(&map_of(Some(map)));
        }
    }
    for (id, presets) in obj.get("presets").and_then(serde_json::Value::as_object).into_iter().flatten() {
        if let Some(t) = tool_by_id(id) {
            let named: BTreeMap<String, Map> = presets
                .as_object()
                .map(|p| p.iter().map(|(name, m)| (name.clone(), map_of(Some(m)))).collect())
                .unwrap_or_default();
            st.presets.insert(t, named);
        }
    }
    if let Some(Ok((width, height, white))) =
        obj.get("last_new").map(|v| serde_json::from_value::<(u32, u32, bool)>(v.clone()))
    {
        let background = if white { Background::White } else { Background::Transparent };
        st.last_new = NewImage { width, height, background };
    }
    if let Some(Ok(recent)) = obj.get("recent_scripts").map(|v| serde_json::from_value::<Vec<PathBuf>>(v.clone())) {
        st.console.recent = recent.into_iter().take(crate::console::RECENT).collect();
    }
    Ok(())
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
        recent_scripts: st.console.recent.clone(),
    };
    let json = serde_json::to_string_pretty(&saved).map_err(std::io::Error::other)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    opal_io::write_atomically(&path, None, |out| opal_io::write_all(out, json.as_bytes(), &path))
        .map_err(std::io::Error::other)
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
            recent_scripts: vec![PathBuf::from("C:/scripts/sky.py")],
        };
        let json = serde_json::to_string(&saved).unwrap();
        let back: Saved = serde_json::from_str(&json).unwrap();
        let mut fresh = AppState::new();
        fresh.prefs.load_map(&back.prefs);
        assert_eq!(fresh.prefs.choice("ui.theme"), "light");
        assert_eq!(back.tools["zoom"].get("zoom.drag"), Some(&Value::Bool(false)));
        apply_json(&mut fresh, &json).unwrap();
        assert_eq!(fresh.console.recent, [PathBuf::from("C:/scripts/sky.py")], "the recent scripts were lost");
        // An empty or partial file still loads.
        let partial: Saved = serde_json::from_str("{}").unwrap();
        assert!(partial.prefs.is_empty());
    }

    #[test]
    fn one_unreadable_value_does_not_lose_the_others() {
        let mut st = AppState::new();
        let text = r#"{
            "prefs": {"ui.theme": {"Choice": "light"}, "ui.scale": {"Gradient": [1, 2]}},
            "tools": {"zoom": {"zoom.drag": {"Bool": false}, "zoom.factor": "nonsense"}},
            "last_new": [640, 480, false]
        }"#;
        apply_json(&mut st, text).unwrap();
        assert_eq!(st.prefs.choice("ui.theme"), "light");
        assert_eq!(st.prefs.f64("ui.scale"), 100.0, "the unreadable value keeps its default");
        assert!(!st.tool_settings[&ToolId::Zoom].bool("zoom.drag"));
        assert_eq!((st.last_new.width, st.last_new.height), (640, 480));
        assert!(apply_json(&mut st, "not json").is_err());
    }
}
