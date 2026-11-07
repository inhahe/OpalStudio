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
    /// The resolution of the last new image, in pixels per inch.
    #[serde(default)]
    last_new_ppi: Option<f64>,
    /// The scripts run lately, the last first.
    #[serde(default)]
    recent_scripts: Vec<PathBuf>,
    /// What was entered lately in each field remembered by name, the latest first.
    #[serde(default)]
    field_entries: BTreeMap<String, Vec<String>>,
}

/// Notes where this program is, in `program.json` beside the settings, for the `opal`
/// package to start it from (with no window, for a script run while no editor is running)
/// when it cannot find it otherwise. Written only when it changed.
pub fn note_program() {
    let Some(path) = settings_path().map(|p| p.with_file_name("program.json")) else { return };
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(exe) = exe.to_str() else { return };
    let json = serde_json::json!({ "path": exe }).to_string();
    if std::fs::read_to_string(&path).is_ok_and(|s| s == json) {
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = opal_io::write_atomically(&path, None, |out| opal_io::write_all(out, json.as_bytes(), &path));
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
        st.last_new = NewImage { width, height, background, resolution: opal_core::units::DEFAULT_PPI };
    }
    if let Some(ppi) = obj.get("last_new_ppi").and_then(serde_json::Value::as_f64)
        && opal_core::units::valid_ppi(ppi)
    {
        st.last_new.resolution = ppi;
    }
    // One by one: one that cannot be read is skipped, not all of them; each once.
    let recent = obj.get("recent_scripts").and_then(serde_json::Value::as_array).into_iter().flatten();
    for path in recent.filter_map(serde_json::Value::as_str).map(PathBuf::from) {
        if !st.console.recent.contains(&path) && st.console.recent.len() < crate::console::RECENT {
            st.console.recent.push(path);
        }
    }
    // So too each field's entries: text that is not blank and could be an expression, each
    // once, as many as a field remembers.
    for (name, list) in obj.get("field_entries").and_then(serde_json::Value::as_object).into_iter().flatten() {
        let mut kept: Vec<String> = Vec::new();
        for e in list.as_array().into_iter().flatten().filter_map(serde_json::Value::as_str) {
            let e = e.trim();
            if !e.is_empty() && e.len() <= opal_core::expr::MAX_LEN && !kept.iter().any(|k| k == e) {
                kept.push(e.to_owned());
            }
        }
        kept.truncate(crate::number_field::REMEMBERED);
        if !kept.is_empty() {
            st.field_entries.insert(name.clone(), kept);
        }
    }
    Ok(())
}

/// Writes the settings, replacing the file only once the new one is complete.
pub fn save(st: &AppState) -> std::io::Result<()> {
    let Some(path) = settings_path() else { return Ok(()) };
    let json = serde_json::to_string_pretty(&saved(st)).map_err(std::io::Error::other)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    opal_io::write_atomically(&path, None, |out| opal_io::write_all(out, json.as_bytes(), &path))
        .map_err(std::io::Error::other)
}

/// What is saved of `st`.
fn saved(st: &AppState) -> Saved {
    Saved {
        prefs: st.prefs.to_map(),
        tools: st
            .tool_settings
            .iter()
            .map(|(t, s)| (t.info().id.to_owned(), s.to_map()))
            .filter(|(_, m)| !m.is_empty())
            .collect(),
        presets: st.presets.iter().map(|(t, p)| (t.info().id.to_owned(), p.clone())).collect(),
        last_new: Some((st.last_new.width, st.last_new.height, st.last_new.background == Background::White)),
        last_new_ppi: Some(st.last_new.resolution),
        // Those JSON can hold (a path that is not Unicode would fail the whole file).
        recent_scripts: st.console.recent.iter().filter(|p| p.to_str().is_some()).cloned().collect(),
        field_entries: st.field_entries.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn a_recent_script_whose_path_is_not_unicode_does_not_keep_the_settings_from_being_saved() {
        use std::os::windows::ffi::OsStringExt;
        let mut st = AppState::new();
        let odd = PathBuf::from(std::ffi::OsString::from_wide(&[0x44, 0x3a, 0x5c, 0xd800, 0x2e, 0x70, 0x79]));
        st.console.recent = vec![odd, PathBuf::from("C:/scripts/fine.py")];
        let json = serde_json::to_string(&saved(&st)).expect("saved all the same");
        assert!(json.contains("fine.py"));
    }

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
            last_new_ppi: None,
            recent_scripts: vec![PathBuf::from("C:/scripts/sky.py")],
            field_entries: BTreeMap::from([("box.x".to_owned(), vec!["Logo.right + 5mm".to_owned(), "10".to_owned()])]),
        };
        let json = serde_json::to_string(&saved).unwrap();
        let back: Saved = serde_json::from_str(&json).unwrap();
        let mut fresh = AppState::new();
        fresh.prefs.load_map(&back.prefs);
        assert_eq!(fresh.prefs.choice("ui.theme"), "light");
        assert_eq!(back.tools["zoom"].get("zoom.drag"), Some(&Value::Bool(false)));
        apply_json(&mut fresh, &json).unwrap();
        assert_eq!(fresh.console.recent, [PathBuf::from("C:/scripts/sky.py")], "the recent scripts were lost");
        assert_eq!(fresh.field_entries["box.x"], ["Logo.right + 5mm", "10"], "the fields' entries were lost");
        // Of entries that cannot be, those that can are kept, each once.
        let mut entries = AppState::new();
        let json = r#"{"field_entries": {"box.y": ["1in", 2, "", "1in", " 3mm "], "box.x": "no"}}"#;
        apply_json(&mut entries, json).unwrap();
        assert_eq!(
            entries.field_entries.get("box.y").map(Vec::as_slice),
            Some(&["1in".to_owned(), "3mm".to_owned()][..])
        );
        assert!(!entries.field_entries.contains_key("box.x"));
        // One that cannot be read is skipped, not all; each kept once.
        let mut odd = AppState::new();
        apply_json(&mut odd, r#"{"recent_scripts": ["C:/a.py", null, "C:/b.py", "C:/a.py"]}"#).unwrap();
        assert_eq!(odd.console.recent, [PathBuf::from("C:/a.py"), PathBuf::from("C:/b.py")]);
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
            "last_new": [640, 480, false],
            "last_new_ppi": 300
        }"#;
        apply_json(&mut st, text).unwrap();
        assert_eq!(st.prefs.choice("ui.theme"), "light");
        assert_eq!(st.prefs.f64("ui.scale"), 100.0, "the unreadable value keeps its default");
        assert!(!st.tool_settings[&ToolId::Zoom].bool("zoom.drag"));
        assert_eq!((st.last_new.width, st.last_new.height, st.last_new.resolution), (640, 480, 300.0));
        // A resolution out of range keeps the default.
        let mut zero = AppState::new();
        apply_json(&mut zero, r#"{"last_new_ppi": 0}"#).unwrap();
        assert_eq!(zero.last_new.resolution, opal_core::units::DEFAULT_PPI);
        assert!(apply_json(&mut st, "not json").is_err());
    }
}
