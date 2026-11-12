//! Image › File Info: what describes the image (title, caption, creators, copyright,
//! keywords), edited as one history state, and how it was taken, shown; templates of the
//! creator's and copyright fields, kept with the settings (design.md, *Metadata*).

use crate::doc::DocId;
use crate::state::AppState;
use eframe::egui::{self, RichText};
use opal_io::metadata::{self, Description};
use serde::{Deserialize, Serialize};

/// What a template fills in: who made an image and holds its copyright, and how to reach
/// them. Empty fields leave File Info's as they are.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Template {
    #[serde(default)]
    pub creators: Vec<String>,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub website: String,
    #[serde(default)]
    pub copyright: String,
    #[serde(default)]
    pub copyrighted: Option<bool>,
    #[serde(default)]
    pub copyright_url: String,
}

impl Template {
    /// The template `d` makes: its creator's and copyright fields.
    pub fn of(d: &Description) -> Template {
        Template {
            creators: d.creators.clone(),
            email: d.email.clone(),
            website: d.website.clone(),
            copyright: d.copyright.clone(),
            copyrighted: d.copyrighted,
            copyright_url: d.copyright_url.clone(),
        }
    }

    /// Fills `d` with what it holds, leaving what it leaves empty.
    pub fn apply(&self, d: &mut Description) {
        if !self.creators.is_empty() {
            d.creators = self.creators.clone();
        }
        for (from, to) in [
            (&self.email, &mut d.email),
            (&self.website, &mut d.website),
            (&self.copyright, &mut d.copyright),
            (&self.copyright_url, &mut d.copyright_url),
        ] {
            if !from.is_empty() {
                to.clone_from(from);
            }
        }
        if self.copyrighted.is_some() {
            d.copyrighted = self.copyrighted;
        }
    }
}

/// The File Info dialog's state.
#[derive(Clone, Debug)]
pub struct FileInfo {
    /// The image it is for.
    pub doc: DocId,
    pub description: Description,
    /// The creators and keywords as typed: separated by semicolons.
    pub creators: String,
    pub keywords: String,
    /// The capture, as shown.
    pub capture: Vec<(&'static str, String)>,
    /// The name a template is saved by, as typed.
    pub template_name: String,
    /// What describes the image as the dialog first showed it, lists as typed: OK with
    /// nothing changed from it changes nothing.
    pub was: Description,
}

/// The texts `typed` lists, separated by semicolons, each trimmed, none empty.
pub fn listed(typed: &str) -> Vec<String> {
    typed.split(';').map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned).collect()
}

impl FileInfo {
    fn of(doc: DocId, meta: &opal_core::document::Metadata) -> FileInfo {
        let description = metadata::description(meta);
        let mut info = FileInfo {
            doc,
            creators: description.creators.join("; "),
            keywords: description.keywords.join("; "),
            description,
            capture: metadata::capture::capture(meta),
            template_name: String::new(),
            was: Description::default(),
        };
        info.was = info.edited();
        info
    }

    /// What describes the image, as edited (the lists as typed).
    pub fn edited(&self) -> Description {
        Description { creators: listed(&self.creators), keywords: listed(&self.keywords), ..self.description.clone() }
    }
}

impl AppState {
    /// Opens File Info on the active image.
    pub(crate) fn open_file_info(&mut self) {
        let Some(id) = self.active_doc else { return };
        let Some(d) = self.doc(id) else { return };
        self.file_info = Some(FileInfo::of(id, &d.doc().metadata));
    }

    /// Makes `d` what describes image `doc` (see `metadata::with_description`), as one history
    /// state labeled "File info"; nothing if it already does. Why not, if it could not.
    pub(crate) fn describe(&mut self, doc: DocId, d: &Description) -> Result<(), String> {
        let image = self.doc_mut(doc).ok_or("The image is closed.")?;
        if metadata::description(&image.doc().metadata) == *d {
            return Ok(());
        }
        let meta = metadata::with_description(&image.doc().metadata, d);
        image
            .apply("File info", |doc, _| {
                doc.metadata = meta;
                Ok::<(), String>(())
            })
            .map_err(|e| e.to_string())
    }
}

/// Draws File Info, while it is open.
pub fn dialog(ctx: &egui::Context, st: &mut AppState) {
    let Some(mut info) = st.file_info.clone() else { return };
    let palette = st.palette;
    let names: Vec<String> = st.metadata_templates.keys().cloned().collect();
    let mut done = None;
    let mut caption_typing = false;
    let mut save_template = false;
    let mut apply: Option<String> = None;
    let mut forget: Option<String> = None;
    egui::Modal::new(egui::Id::unique("file-info")).show(ctx, |ui| {
        ui.heading("File Info");
        ui.label(
            RichText::new(
                "What describes the image, kept in its file and written on export as Export's Metadata \
                 setting says. Separate several creators or keywords with semicolons.",
            )
            .small()
            .color(palette.weak_text),
        );
        ui.add_space(6.0);
        let d = &mut info.description;
        egui::Grid::new("file-info-description").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            let line = |ui: &mut egui::Ui, label: &str, text: &mut String| {
                let l = ui.label(label);
                ui.add(egui::TextEdit::singleline(text).desired_width(320.0)).labelled_by(l.id);
                ui.end_row();
            };
            line(ui, "Title", &mut d.title);
            let l = ui.label("Caption");
            let caption = ui
                .add(egui::TextEdit::multiline(&mut d.caption).desired_width(320.0).desired_rows(3))
                .labelled_by(l.id);
            caption_typing = caption.has_focus();
            ui.end_row();
            line(ui, "Creators", &mut info.creators);
            line(ui, "Creator's email", &mut d.email);
            line(ui, "Creator's website", &mut d.website);
            line(ui, "Copyright notice", &mut d.copyright);
            let status_label = ui.label("Copyright status");
            let status = |c: Option<bool>| match c {
                None => "Not said",
                Some(true) => "Copyrighted",
                Some(false) => "Public domain",
            };
            egui::ComboBox::from_id_salt("file-info-status")
                .selected_text(status(d.copyrighted))
                .show_ui(ui, |ui| {
                    for c in [None, Some(true), Some(false)] {
                        ui.selectable_value(&mut d.copyrighted, c, status(c));
                    }
                })
                .response
                .labelled_by(status_label.id);
            ui.end_row();
            line(ui, "Copyright web page", &mut d.copyright_url);
            line(ui, "Keywords", &mut info.keywords);
        });
        ui.add_space(6.0);
        // Templates: the creator's and copyright fields, kept by name.
        ui.horizontal(|ui| {
            let template_label = ui.label("Template");
            ui.add_enabled_ui(!names.is_empty(), |ui| {
                egui::ComboBox::from_id_salt("file-info-template")
                    .selected_text("Apply…")
                    .show_ui(ui, |ui| {
                        for n in &names {
                            if ui.selectable_label(false, n).clicked() {
                                apply = Some(n.clone());
                            }
                        }
                    })
                    .response
                    .labelled_by(template_label.id);
            });
            let name_label = ui.label("Name");
            ui.add(egui::TextEdit::singleline(&mut info.template_name).desired_width(110.0)).labelled_by(name_label.id);
            let named = !info.template_name.trim().is_empty();
            if ui.add_enabled(named, egui::Button::new("Save as template")).clicked() {
                save_template = true;
            }
            if named
                && names.iter().any(|n| n == info.template_name.trim())
                && ui.button("Forget").on_hover_text("Forget the template of this name").clicked()
            {
                forget = Some(info.template_name.trim().to_owned());
            }
        });
        ui.label(
            RichText::new(
                "A template keeps the creators, contact and copyright fields; applied, it fills those it has.",
            )
            .small()
            .color(palette.weak_text),
        );
        if !info.capture.is_empty() {
            ui.add_space(6.0);
            egui::CollapsingHeader::new("Capture").default_open(true).show(ui, |ui| {
                egui::Grid::new("file-info-capture").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                    for (label, value) in &info.capture {
                        ui.label(RichText::new(*label).color(palette.weak_text));
                        ui.label(value);
                        ui.end_row();
                    }
                });
            });
        }
        // Enter in the caption begins a line; elsewhere, it is OK.
        done = crate::dialogs::ok_cancel_unless(ui, "OK", caption_typing);
    });
    if let Some(name) = apply
        && let Some(t) = st.metadata_templates.get(&name)
    {
        let mut d = info.edited();
        t.apply(&mut d);
        info.creators = d.creators.join("; ");
        info.description = d;
    }
    if save_template {
        let name = info.template_name.trim().to_owned();
        st.metadata_templates.insert(name, Template::of(&info.edited()));
        st.settings_touched();
    }
    if let Some(name) = forget {
        st.metadata_templates.remove(&name);
        st.settings_touched();
    }
    match done {
        Some(true) => {
            st.file_info = None;
            let edited = info.edited();
            if edited == info.was {
                // Nothing changed (lists split as typed, which may not be as the file had them).
            } else if let Err(why) = st.describe(info.doc, &edited) {
                st.message(crate::state::MessageKind::Error, "File Info", why);
            }
        }
        Some(false) => st.file_info = None,
        None => st.file_info = Some(info),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_template_fills_what_it_holds_and_leaves_the_rest() {
        let mut d = Description { title: "Harbour".into(), email: "old@example.com".into(), ..Description::default() };
        let t = Template {
            creators: vec!["Ann".into()],
            copyright: "© Ann".into(),
            copyrighted: Some(true),
            ..Template::default()
        };
        t.apply(&mut d);
        assert_eq!((d.title.as_str(), d.email.as_str()), ("Harbour", "old@example.com"));
        assert_eq!(
            (d.creators.clone(), d.copyright.as_str(), d.copyrighted),
            (vec!["Ann".to_owned()], "© Ann", Some(true))
        );
        assert_eq!(Template::of(&d).copyright, "© Ann");
        assert_eq!(listed(" sea ; ;boat;"), ["sea", "boat"]);
    }
}
