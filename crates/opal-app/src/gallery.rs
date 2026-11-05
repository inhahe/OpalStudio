//! The widget gallery: shared controls on their own, for trying them out.

use crate::curve_editor::{self, Options};
use eframe::egui;
use opal_color::ColorModel;
use opal_core::curve::Curve;

pub struct Gallery {
    curve: Curve,
    model: ColorModel,
    curves: [Curve; 3],
    channel: usize,
}

impl Default for Gallery {
    fn default() -> Gallery {
        let model = ColorModel::Oklch;
        let chans = model.channels();
        Gallery {
            curve: Curve::identity(),
            model,
            curves: std::array::from_fn(|i| curve_editor::default_channel_curve(chans[i].kind)),
            channel: 0,
        }
    }
}

impl Gallery {
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Curve editor");
        ui.label("Drag to add or move points; right-click or double-click a point to remove it.");
        let hist: Vec<f32> = (0..64).map(|i| ((i as f32 / 10.0).sin() * 0.4 + 0.5).abs()).collect();
        let opts = Options { histogram: Some(&hist), ..Options::large("gallery-curve") };
        curve_editor::curve_editor(ui, &mut self.curve, &opts);
        ui.add_space(12.0);
        ui.heading("Channel curve editor");
        ui.label("One curve per channel, in a color model you pick. Hue channels wrap around.");
        curve_editor::channel_curves(ui, "gallery-channels", &mut self.model, &mut self.curves, &mut self.channel);
    }
}
