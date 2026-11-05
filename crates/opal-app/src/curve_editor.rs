//! The curve editor, and the channel curve editor built on it.
//!
//! Drag a point to move it; drag empty space to add a point there and move it; right-click
//! or double-click a point to remove it.

use eframe::egui::{self, Color32, Pos2, Rect, Response, Sense, Stroke, Vec2};
use opal_color::{ChannelKind, ColorModel};
use opal_core::curve::{Curve, Interpolation};

/// How a curve editor looks.
pub struct Options<'a> {
    /// Distinguishes this editor's interaction state from other editors'.
    pub id_salt: &'a str,
    pub size: Vec2,
    /// Bar heights (0–1) drawn behind the curve, evenly spaced from x = 0 to 1.
    pub histogram: Option<&'a [f32]>,
    /// The color at each x, drawn as a strip below the graph.
    pub x_gradient: Option<&'a dyn Fn(f64) -> Color32>,
    /// Whether to offer the interpolation choice below the graph.
    pub show_interpolation: bool,
}

impl<'a> Options<'a> {
    /// A compact editor, as in a settings row.
    pub fn small(id_salt: &'a str) -> Options<'a> {
        Options { id_salt, size: egui::vec2(180.0, 120.0), histogram: None, x_gradient: None, show_interpolation: true }
    }

    /// A roomy editor, as in a dialog.
    pub fn large(id_salt: &'a str) -> Options<'a> {
        Options { size: egui::vec2(300.0, 220.0), ..Options::small(id_salt) }
    }
}

const GRAB_RADIUS: f32 = 7.0;
const GRADIENT_HEIGHT: f32 = 10.0;

/// Shows an editable curve. The response is marked changed when the curve changes.
pub fn curve_editor(ui: &mut egui::Ui, curve: &mut Curve, opts: &Options<'_>) -> Response {
    let mut changed = false;
    let inner = ui.vertical(|ui| {
        let extra = if opts.x_gradient.is_some() { GRADIENT_HEIGHT + 2.0 } else { 0.0 };
        let (outer, response) = ui.allocate_exact_size(opts.size + egui::vec2(0.0, extra), Sense::click_and_drag());
        let rect = Rect::from_min_size(outer.min, opts.size).shrink(4.0);
        let id = response.id;
        let to_screen = |p: [f64; 2]| {
            Pos2::new(rect.left() + p[0] as f32 * rect.width(), rect.bottom() - p[1] as f32 * rect.height())
        };
        let to_curve =
            |p: Pos2| [f64::from((p.x - rect.left()) / rect.width()), f64::from((rect.bottom() - p.y) / rect.height())];
        let nearest = |pos: Pos2, c: &Curve| {
            c.points()
                .iter()
                .enumerate()
                .map(|(i, p)| (i, to_screen(*p).distance(pos)))
                .filter(|(_, d)| *d <= GRAB_RADIUS)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
        };

        // Interaction.
        let pointer = response.interact_pointer_pos().or(response.hover_pos());
        let mut dragging: Option<usize> = ui.data(|d| d.get_temp(id));
        if response.drag_started()
            && let Some(pos) = response.interact_pointer_pos()
        {
            dragging = Some(match nearest(pos, curve) {
                Some(i) => i,
                None => {
                    let [x, y] = to_curve(pos);
                    changed = true;
                    curve.insert(x, y)
                }
            });
        }
        if response.dragged()
            && let (Some(i), Some(pos)) = (dragging, response.interact_pointer_pos())
            && i < curve.points().len()
        {
            let [x, y] = to_curve(pos);
            if curve.points()[i] != [x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)] {
                curve.move_point(i, x, y);
                changed = true;
            }
        }
        if response.drag_stopped() {
            dragging = None;
        }
        if (response.secondary_clicked() || response.double_clicked())
            && let Some(pos) = response.interact_pointer_pos()
            && let Some(i) = nearest(pos, curve)
            && curve.remove(i)
        {
            changed = true;
            dragging = None;
        }
        ui.data_mut(|d| match dragging {
            Some(i) => {
                d.insert_temp(id, i);
            }
            None => d.remove::<usize>(id),
        });

        // Drawing.
        let painter = ui.painter_at(outer);
        let visuals = ui.visuals();
        painter.rect_filled(rect.expand(4.0), 4.0, visuals.extreme_bg_color);
        let grid = Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color.gamma_multiply(0.6));
        for k in 1..4 {
            let t = k as f32 / 4.0;
            painter.line_segment([rect.lerp_inside(egui::vec2(t, 0.0)), rect.lerp_inside(egui::vec2(t, 1.0))], grid);
            painter.line_segment([rect.lerp_inside(egui::vec2(0.0, t)), rect.lerp_inside(egui::vec2(1.0, t))], grid);
        }
        if let Some(hist) = opts.histogram
            && !hist.is_empty()
        {
            let w = rect.width() / hist.len() as f32;
            let fill = visuals.weak_text_color().gamma_multiply(0.25);
            for (i, h) in hist.iter().enumerate() {
                let x = rect.left() + i as f32 * w;
                let top = rect.bottom() - h.clamp(0.0, 1.0) * rect.height();
                painter.rect_filled(Rect::from_min_max(Pos2::new(x, top), Pos2::new(x + w, rect.bottom())), 0.0, fill);
            }
        }
        let samples = (rect.width() as usize).max(2);
        let line: Vec<Pos2> = (0..=samples)
            .map(|s| {
                let x = s as f64 / samples as f64;
                to_screen([x, curve.eval(x)])
            })
            .collect();
        let accent = visuals.selection.stroke.color;
        painter.add(egui::Shape::line(line, Stroke::new(2.0, accent)));
        let hovered = pointer.and_then(|p| nearest(p, curve));
        for (i, p) in curve.points().iter().enumerate() {
            let c = to_screen(*p);
            let active = dragging == Some(i) || hovered == Some(i);
            let r = if active { 5.5 } else { 4.0 };
            painter.circle(
                c,
                r,
                if dragging == Some(i) { accent } else { visuals.extreme_bg_color },
                Stroke::new(1.5, accent),
            );
        }
        if let Some(g) = opts.x_gradient {
            let strip = Rect::from_min_size(
                Pos2::new(rect.left(), outer.top() + opts.size.y + 2.0),
                egui::vec2(rect.width(), GRADIENT_HEIGHT),
            );
            let steps = (strip.width() / 2.0).max(1.0) as usize;
            for s in 0..steps {
                let x0 = strip.left() + s as f32 * strip.width() / steps as f32;
                let x1 = strip.left() + (s + 1) as f32 * strip.width() / steps as f32;
                let col = g((s as f64 + 0.5) / steps as f64);
                painter.rect_filled(
                    Rect::from_min_max(Pos2::new(x0, strip.top()), Pos2::new(x1, strip.bottom())),
                    0.0,
                    col,
                );
            }
        }
        let shown = hovered.or(dragging).and_then(|i| curve.points().get(i).copied());
        let response = match shown {
            Some([x, y]) => response.on_hover_text(format!("{x:.3} → {y:.3}")),
            None => response.on_hover_text("Drag to add or move a point; right-click a point to remove it."),
        };

        if opts.show_interpolation {
            let label = |i: Interpolation| match i {
                Interpolation::Linear => "Linear",
                Interpolation::Smooth => "Smooth (no overshoot)",
                Interpolation::Spline => "Spline (may overshoot)",
            };
            egui::ComboBox::from_id_salt(("interp", opts.id_salt)).selected_text(label(curve.interpolation)).show_ui(
                ui,
                |ui| {
                    for i in [Interpolation::Smooth, Interpolation::Linear, Interpolation::Spline] {
                        if ui.selectable_label(curve.interpolation == i, label(i)).clicked() && curve.interpolation != i
                        {
                            curve.interpolation = i;
                            changed = true;
                        }
                    }
                },
            );
        }
        response
    });
    let mut response = inner.inner;
    if changed {
        response.mark_changed();
    }
    response
}

/// The curve a channel starts with: flat at 1 (everything passes) for hue channels, which
/// wrap, and `y = x` otherwise.
pub fn default_channel_curve(kind: ChannelKind) -> Curve {
    match kind {
        ChannelKind::Hue => Curve::new(&[[0.0, 1.0], [0.5, 1.0]], Interpolation::Smooth, true),
        _ => Curve::identity(),
    }
}

/// The color at position `x` (0–1 across the channel's range) of channel `c` of `model`,
/// holding the other channels at a mid-gray, or at a vivid color for a hue channel.
pub fn channel_ramp(model: ColorModel, c: usize, x: f64) -> Color32 {
    let chans = model.channels();
    let mut v = model.from_linear_srgb([0.214, 0.214, 0.214]);
    if chans[c].kind == ChannelKind::Hue {
        match model {
            ColorModel::Hsb => v = [0.0, 100.0, 100.0],
            ColorModel::Hsl => v = [0.0, 100.0, 50.0],
            ColorModel::Lch => v = [60.0, 70.0, 0.0],
            ColorModel::Oklch => v = [0.7, 0.15, 0.0],
            _ => {}
        }
    }
    v[c] = chans[c].min + x * (chans[c].max - chans[c].min);
    let rgb = model.to_linear_srgb(v);
    let enc = |l: f64| (opal_color::srgb_encode(l.clamp(0.0, 1.0)) * 255.0).round() as u8;
    Color32::from_rgb(enc(rgb[0]), enc(rgb[1]), enc(rgb[2]))
}

/// One curve per channel of a color model the user picks. Changing the model resets the
/// curves (their meaning would change). Returns whether anything changed.
pub fn channel_curves(
    ui: &mut egui::Ui,
    id_salt: &str,
    model: &mut ColorModel,
    curves: &mut [Curve; 3],
    channel: &mut usize,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("Color model");
        egui::ComboBox::from_id_salt(("model", id_salt)).selected_text(model.name()).show_ui(ui, |ui| {
            for m in ColorModel::ALL {
                let r = ui.selectable_label(*model == m, m.name()).on_hover_text(m.description());
                if r.clicked() && *model != m {
                    *model = m;
                    for (i, ch) in m.channels().iter().enumerate() {
                        curves[i] = default_channel_curve(ch.kind);
                    }
                    changed = true;
                }
            }
        });
    });
    let chans = model.channels();
    ui.horizontal(|ui| {
        for (i, ch) in chans.iter().enumerate() {
            ui.selectable_value(channel, i, ch.name);
        }
    });
    let c = (*channel).min(2);
    let m = *model;
    let ramp = move |x: f64| channel_ramp(m, c, x);
    let salt = format!("{id_salt}-{}-{c}", m.id());
    let opts = Options { x_gradient: Some(&ramp), ..Options::large(&salt) };
    if curve_editor(ui, &mut curves[c], &opts).changed() {
        changed = true;
    }
    let ch = chans[c];
    ui.label(
        egui::RichText::new(format!("{}: {} to {}", ch.name, ch.min, ch.max))
            .small()
            .color(ui.visuals().weak_text_color()),
    );
    changed
}
