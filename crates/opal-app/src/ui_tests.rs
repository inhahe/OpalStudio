//! Tests of the whole application through its interface: simulated input on the real app,
//! rendered with wgpu as it would be on screen.

use crate::app::OpalApp;
use crate::commands::Cmd;
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

/// Held by each test while it uses the GPU. The harness renders with WARP, Windows' software
/// rasterizer, which crashed (an access violation in its shader compiler) when several
/// tests compiled shaders on their own devices at the same time.
fn gpu_lock() -> std::sync::MutexGuard<'static, ()> {
    static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());
    GPU.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn harness() -> Harness<'static, OpalApp> {
    harness_of([1400.0, 860.0])
}

/// A harness whose window is `size` points.
fn harness_of(size: [f32; 2]) -> Harness<'static, OpalApp> {
    Harness::builder().with_size(size).with_max_steps(12).wgpu().build_eframe(|cc| OpalApp::for_tests(cc))
}

/// Saves the harness's current rendering as `<name>.png` in the temporary folder (for
/// looking at during development) and returns the image.
fn render(h: &mut Harness<'static, OpalApp>, name: &str) -> image::RgbaImage {
    let img = h.render().expect("the app renders");
    let path = std::env::temp_dir().join(format!("opal-ui-{name}.png"));
    img.save(&path).expect("the rendering saves");
    img
}

#[test]
fn the_new_image_dialog_creates_a_document() {
    let _gpu = gpu_lock();
    let mut h = harness();
    h.run();
    h.get_by_label("New").click();
    h.run();
    h.get_by_label("Create").click();
    h.run();
    let st = h.state().state();
    assert_eq!(st.docs.len(), 1);
    assert_eq!(st.active().unwrap().doc().canvas.width, st.last_new.width);
}

#[test]
fn a_paper_size_makes_an_image_of_its_size_at_its_resolution() {
    let _gpu = gpu_lock();
    let mut h = harness();
    h.state_mut().state_mut().set_unit(opal_core::units::Unit::In);
    h.run();
    h.get_by_label("New").click();
    h.run();
    h.get_by_label("Letter").click();
    h.run();
    render(&mut h, "new-image-letter");
    h.get_by_label("Create").click();
    h.run();
    let d = h.state().state().active().unwrap().doc();
    assert_eq!((d.canvas.width, d.canvas.height, d.resolution), (2550, 3300, 300.0));
    // Resize Image shows it in inches, and how large it prints.
    h.state_mut().state_mut().queue.push(Cmd::ResizeImage);
    h.run();
    assert!(h.query_by_label_contains("prints 8.5 × 11 in").is_some(), "the print size is not told");
    render(&mut h, "resize-image-inches");
}

#[test]
fn the_rulers_take_a_strip_beside_the_view_and_ctrl_r_hides_them() {
    let _gpu = gpu_lock();
    let mut h = harness();
    new_image(&mut h, 400, 300, 100.0);
    h.state_mut().state_mut().set_unit(opal_core::units::Unit::In);
    h.run();
    let view = |h: &Harness<'static, OpalApp>| h.state().state().active().unwrap().view.screen_rect;
    let with = view(&h);
    render(&mut h, "rulers");
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::R);
    h.run();
    assert!(!h.state().state().prefs.bool("canvas.rulers"), "Ctrl+R did not hide them");
    let without = view(&h);
    let inset = crate::rulers::THICK;
    assert_eq!((with.min - without.min), egui::vec2(inset, inset), "the rulers took no strip");
    assert_eq!(with.max, without.max);
}

#[test]
fn counting_from_the_layer_being_moved_counts_from_where_it_was() {
    let _gpu = gpu_lock();
    let mut h = harness();
    new_image(&mut h, 1000, 300, 100.0);
    // A paste put at a fractional place left of the canvas (x -20.3) and applied: a layer
    // whose bounds are fractional.
    put_on_clipboard(&mut h, pasted(50, 40, [1.0, 0.0, 0.0, 1.0]));
    h.event(egui::Event::Paste(String::new()));
    h.run();
    {
        let d = h.state_mut().state_mut().active_mut().unwrap();
        let m = d.transform.as_ref().unwrap().current.then(opal_core::geom::Affine::translate(-495.3, 0.0));
        d.set_box(m, None);
    }
    h.key_press(egui::Key::Enter);
    h.run();
    let layer = h.state().state().active().unwrap().active_layer().unwrap();
    // Counting from its own top-left corner, it is transformed with the fields open, its
    // position given at its center (where shown from a fractional point, a number shown
    // does not come back the same).
    {
        let st = h.state_mut().state_mut();
        st.set_unit(opal_core::units::Unit::In);
        st.prefs.set("transform.reference", crate::settings::Value::Choice("center".into()));
        st.box_numbers = true;
        st.active_mut().unwrap().origin =
            crate::box_fields::Origin { base: crate::box_fields::Base::Layer(layer), point: 0 };
    }
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::T);
    h.run();
    for _ in 0..10 {
        h.key_press(egui::Key::ArrowRight);
    }
    h.run();
    let d = h.state().state().active().unwrap();
    assert!(d.transform.is_some(), "the box did not open");
    assert_eq!(d.origin.base, crate::box_fields::Base::Layer(layer), "the choice was dropped while it moved");
    assert_eq!(d.fit_rect, None, "a rectangle was kept though none was typed");
    // Its center moved 10 px from where it was: 35 px (0.35 in) right of where its top-left
    // corner was, which is what the fields count from.
    let shows = |h: &Harness<'static, OpalApp>, v: f64| {
        h.query_all(
            egui_kittest::kittest::by().predicate(move |n| n.numeric_value().is_some_and(|x| (x - v).abs() < 1e-6)),
        )
        .count()
    };
    assert!(shows(&h, 0.35) >= 1, "not counted from where the layer was");
}

#[test]
fn the_print_size_button_on_a_placed_file_switches_it_and_is_remembered() {
    let _gpu = gpu_lock();
    let mut h = harness();
    new_image(&mut h, 400, 300, 100.0);
    // A 90-pixel file at 300 ppi: 30 pixels at its print size here.
    let path = std::env::temp_dir().join(format!("opal-ui-place-{}.jpg", std::process::id()));
    crate::paste::testing::white_jpeg(&path, 90, 60, Some([300, 300]));
    h.state_mut().state_mut().place_file(&path, None, true).unwrap();
    std::fs::remove_file(&path).ok();
    h.run();
    let width = |h: &Harness<'static, OpalApp>| box_params(h).width;
    assert_eq!(width(&h), 30.0);
    h.get_by_label(&crate::icons::PRINT_SIZE.to_string()).click();
    h.run();
    assert_eq!(width(&h), 90.0, "the button did not switch it to pixel for pixel");
    assert!(!h.state().state().prefs.bool("transform.place_print_size"), "the choice was not remembered");
}

/// In the starting layout, the tool settings (cut short above the layers) show that there is
/// more of them: their scroll bar shows without the pointer on it.
#[test]
fn a_panel_cut_short_shows_its_scroll_bar() {
    let _gpu = gpu_lock();
    let mut h = harness();
    pasting(&mut h);
    h.run();
    // Marks unlike the panel in the strip kept for the bar at the pane's right edge, beside
    // its first rows (where the handle is, nothing scrolled yet).
    let pane = h.state().pane_rect(crate::app::Pane::ToolSettings);
    let strip = h.ctx.global_style().spacing.scroll.allocated_width();
    let panel = h.ctx.global_style().visuals.panel_fill;
    let img = render(&mut h, "scroll-bar");
    let px = |v: f32| (v * h.ctx.pixels_per_point()) as u32;
    let bar = (px(pane.right() - strip)..px(pane.right()))
        .flat_map(|x| (px(pane.top())..px(pane.top() + 40.0)).map(move |y| (x, y)))
        .filter(|&(x, y)| img.get_pixel(x, y)[0].abs_diff(panel.r()) > 30)
        .count();
    assert!(bar > 0, "no scroll bar shows right of the tool settings");
}

/// Asserts that the tool settings show each of `fields` (a name, and the value its field
/// shows) whole, without scrolling.
fn in_the_tool_settings(h: &Harness<'static, OpalApp>, fields: &[(&str, f64)]) {
    let pane = h.state().pane_rect(crate::app::Pane::ToolSettings);
    for &(what, v) in fields {
        let field = h.get(field_showing(v, 1e-6, false)).rect();
        assert!(pane.contains_rect(field), "the {what} field is out of sight: {field:?}, the pane {pane:?}");
    }
}

/// In the starting layout, the tool settings (short there, above the layers) show a paste's
/// position and size without scrolling.
#[test]
fn the_tool_settings_show_a_paste_s_numbers_without_scrolling() {
    let _gpu = gpu_lock();
    let mut h = harness();
    pasting(&mut h);
    h.run();
    let p = box_params(&h);
    in_the_tool_settings(&h, &[("x", p.x), ("y", p.y), ("width", p.width), ("height", p.height)]);
}

/// In the starting layout, in a window maximized on a full-HD screen at 125% (864 points tall,
/// less the taskbar's 48 and the title bar's 31 or so), the tool settings show the crop box's
/// size without scrolling, after the crop tool's own settings.
#[test]
fn the_tool_settings_show_the_crop_box_s_numbers_without_scrolling() {
    let _gpu = gpu_lock();
    let mut h = harness_of([1400.0, 780.0]);
    h.state_mut().state_mut().set_tool(crate::tools::ToolId::Crop);
    image_of(&mut h, 400, 300);
    h.run();
    in_the_tool_settings(&h, &[("width", 400.0), ("height", 300.0)]);
}

/// What a tool or the transform box does is said by the status bar, and by the heading of the
/// tool settings on hover, not in them.
#[test]
fn the_status_bar_and_the_heading_say_what_the_canvas_does() {
    use egui_kittest::kittest::by;
    let _gpu = gpu_lock();
    let mut h = harness();
    h.state_mut().state_mut().set_tool(crate::tools::ToolId::Crop);
    image_of(&mut h, 400, 300);
    h.run();
    // Where it shows (egui also gives each hover text to screen readers, in a node not drawn).
    let said = |h: &Harness<'static, OpalApp>, does: &str| {
        let shown = by().label_contains(does).predicate(|n| n.bounding_box().is_some());
        h.query_all(shown).map(|n| n.rect()).collect::<Vec<_>>()
    };
    let crop = crate::tools::ToolId::Crop.info().description;
    let pane = h.state().pane_rect(crate::app::Pane::ToolSettings);
    let in_status_bar = |r: &egui::Rect| r.top() > pane.bottom() && r.left() < pane.left();
    let shown = said(&h, crop);
    assert!(shown.len() == 1 && in_status_bar(&shown[0]), "the crop tool's description: {shown:?}");
    // On the heading, it shows on hover.
    h.get(on_side(by().label("Crop").role(egui::accesskit::Role::Label), false)).hover();
    for _ in 0..8 {
        h.step();
    }
    assert_eq!(said(&h, crop).len(), 2, "the heading does not say on hover what the tool does");
    // While a box is open, the canvas moves it, and the status bar says how.
    h.event(egui::Event::PointerGone);
    put_on_clipboard(&mut h, pasted(50, 40, [1.0, 0.0, 0.0, 1.0]));
    h.event(egui::Event::Paste(String::new()));
    h.run();
    let shown = said(&h, crate::transform_ui::HINT);
    assert!(shown.len() == 1 && in_status_bar(&shown[0]), "the box's hint: {shown:?}");
    // And the box's heading says it on hover.
    h.get(on_side(by().label("Paste").role(egui::accesskit::Role::Label), false)).hover();
    for _ in 0..8 {
        h.step();
    }
    assert_eq!(said(&h, crate::transform_ui::HINT).len(), 2, "the box's heading does not say on hover how it is used");
}

/// The window is put all where windows go on its monitor, once: left where it is if it is,
/// moved there if it fits, maximized if not; nothing done while it is minimized; and in the
/// interface's zoom as in none.
#[test]
fn the_window_is_put_where_windows_go_on_its_monitor() {
    use egui::{Rect, ViewportCommand, pos2, vec2};
    let _gpu = gpu_lock();
    // A full-HD screen's room for windows, and a 1440p one's, in physical pixels.
    let full_hd = crate::app::full_hd_work_area;
    fn qhd(_: egui::Pos2) -> Option<Rect> {
        Some(Rect::from_min_max(pos2(0.0, 0.0), pos2(2560.0, 1400.0)))
    }
    // The commands asked for in two frames, on a monitor whose room is `room`, of a window whose
    // content is `content` physical pixels at `at` (its frame's corner: a title bar of 31
    // above, borders of 8 unseen about the rest), in an interface zoomed `zoom`, minimized
    // for the first frame if `minimized`.
    let asked = |content: [f32; 2], at: [f32; 2], room: fn(egui::Pos2) -> Option<Rect>, zoom: f32, minimized: bool| {
        let mut h = harness();
        h.state_mut().set_work_area(room);
        h.ctx.set_zoom_factor(zoom);
        h.run();
        let inner = Rect::from_min_size(pos2(at[0] + 8.0, at[1] + 31.0), content.into());
        let outer = Rect::from_min_max(at.into(), inner.max + vec2(8.0, 8.0));
        let in_points = |r: Rect| Rect::from_min_max(r.min / zoom, r.max / zoom);
        let mut frames = Vec::new();
        for frame in 0..3 {
            let root = h.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default();
            root.outer_rect = Some(in_points(outer));
            root.inner_rect = Some(in_points(inner));
            root.minimized = Some(minimized && frame == 0);
            h.step();
            let output = h.output().viewport_output.get(&egui::ViewportId::ROOT);
            let cmds = output.map(|v| v.commands.clone()).unwrap_or_default();
            frames.push(
                cmds.into_iter()
                    .filter(|c| matches!(c, ViewportCommand::Maximized(_) | ViewportCommand::OuterPosition(_)))
                    .collect::<Vec<_>>(),
            );
        }
        assert!(frames[2].is_empty(), "asked again: {frames:?}");
        if minimized {
            assert!(frames[0].is_empty(), "asked while minimized: {frames:?}");
        }
        frames.into_iter().flatten().collect::<Vec<_>>()
    };
    let start = crate::app::START_SIZE;
    // In the middle of a full-HD screen at 100%: left where it is.
    assert_eq!(asked(start, [232.0, 50.0], full_hd, 1.0, false), []);
    // Cascaded down, its bottom under the taskbar: moved into the middle, once restored.
    let moved = asked(start, [300.0, 150.0], full_hd, 1.0, true);
    let shown = vec2(start[0], start[1] + 31.0);
    let corner = pos2(960.0, 516.0) - shown / 2.0 - vec2(8.0, 0.0);
    assert_eq!(moved, [ViewportCommand::OuterPosition(corner)], "not moved into the room");
    // At 125% (its content a quarter larger): too tall, maximized.
    let large = [start[0] * 1.25, start[1] * 1.25];
    assert_eq!(asked(large, [0.0, 0.0], full_hd, 1.0, false), [ViewportCommand::Maximized(true)]);
    // On a 1440p screen, the interface zoomed 125%: the window fits, and is left.
    assert_eq!(asked(start, [560.0, 200.0], qhd, 1.25, false), [], "the zoom made it not fit");
}

/// What the status bar's row shows in a window `height` points tall, in the order it is read
/// out: the first (its text, which `says` begins, as the test asserts), and the parts after it;
/// each as what it says and where.
fn status_bar(h: &Harness<'static, OpalApp>, height: f32, says: &str) -> (egui::Rect, Vec<(String, egui::Rect)>) {
    use egui_kittest::kittest::{NodeT, by};
    let in_bar = by().predicate(move |n| {
        n.bounding_box().is_some_and(|b| b.y0 > f64::from(height) - 30.0 && b.y1 <= f64::from(height))
    });
    // A label's text is its value.
    let text = |n: &egui_kittest::Node| {
        let a = n.accesskit_node();
        a.label().or(a.value()).unwrap_or_default()
    };
    let mut shown: Vec<(String, egui::Rect)> =
        h.query_all(in_bar).map(|n| (text(&n), n.rect())).filter(|(t, _)| !t.is_empty()).collect();
    assert!(!shown.is_empty(), "the status bar shows nothing");
    let (first, said) = shown.remove(0);
    assert!(first.contains(says), "what is read first is not the text: {first:?}, then {shown:?}");
    (said, shown)
}

/// The text of the status bar while a paste's box is open (its start).
fn box_hint() -> &'static str {
    &crate::transform_ui::HINT[..20]
}

/// In a window as narrow as one can be, what the canvas does is read out first in the status
/// bar, and shows, cut short, left of its parts, which leave it room by leaving out the least
/// needed of them.
#[test]
fn the_status_bar_cuts_short_what_does_not_fit() {
    let _gpu = gpu_lock();
    let mut h = harness_of([720.0, 600.0]);
    pasting(&mut h);
    // In millimetres, the canvas's size is long, and with the pointer's place too the parts do
    // not all fit.
    h.state_mut().state_mut().set_unit(opal_core::units::Unit::Mm);
    h.event(egui::Event::PointerMoved(egui::pos2(250.0, 300.0)));
    h.run();
    assert!(h.state().state().gpu.memory.is_some(), "no GPU memory to leave out");
    assert!(h.state().state().cursor_shown.is_some(), "the pointer is not over the canvas");
    let (hint, parts) = status_bar(&h, 600.0, box_hint());
    assert!(hint.width() >= 155.0, "the parts leave it too little room: {hint:?}");
    for (label, r) in &parts {
        assert!(r.left() >= hint.right(), "{label:?} runs over what the canvas does: {r:?} {hint:?}");
    }
    assert!(!parts.iter().any(|(l, _)| l.starts_with("GPU")), "the least needed part was kept: {parts:?}");
    assert!(parts.iter().any(|(l, _)| l.ends_with("ppi")), "a part more needed was left out: {parts:?}");
}

/// About the width where the status bar's parts no longer all fit, the text keeps its room, no
/// part runs over it, and which parts show holds from frame to frame: a part is as wide
/// wherever it stands (first, or after a separator).
#[test]
fn the_status_bar_holds_about_where_a_part_is_left_out() {
    let _gpu = gpu_lock();
    let mut h = harness();
    pasting(&mut h);
    // Parts of widths that hold: the size in millimetres, the GPU's memory fixed, no scripts and
    // no tiles on disk (whatever other tests do with the tile store).
    h.state_mut().state_mut().set_unit(opal_core::units::Unit::Mm);
    h.state_mut().pin_gpu_memory((212 << 20, 48 << 30));
    h.state_mut().pretend_status(0, 0);
    h.run();
    let gap = h.ctx.global_style().spacing.item_spacing.x;
    let [width, height] = [h.ctx.content_rect().width(), h.ctx.content_rect().height()];
    let room = crate::app::STATUS_TEXT_ROOM;
    // Where they would just fit: the text's start, its room and the gap after it, and the parts
    // from where they begin to the window's edge.
    let (said, parts) = status_bar(&h, height, box_hint());
    assert!(parts.iter().any(|(t, _)| t.starts_with("GPU")), "no GPU part: {parts:?}");
    let left = parts.iter().map(|(_, r)| r.left()).fold(f32::INFINITY, f32::min);
    let fits = said.left() + room + gap + (width - left);
    for i in 0..14 {
        let width = (fits - 20.0 + 2.0 * i as f32).round();
        h.set_size(egui::vec2(width, height));
        h.run();
        let mut seen = Vec::new();
        for _ in 0..3 {
            h.step();
            let (said, parts) = status_bar(&h, height, box_hint());
            let left = parts.iter().map(|(_, r)| r.left()).fold(f32::INFINITY, f32::min);
            assert!(said.right() <= left - gap + 0.5, "at {width}, a part runs over the text: {said:?} {parts:?}");
            let has = left - gap - said.left();
            assert!(has >= room - 0.5, "at {width}, the text has {has} points: {parts:?}");
            seen.push(parts.iter().map(|(t, _)| t.clone()).collect::<Vec<_>>());
        }
        assert!(seen.windows(2).all(|w| w[0] == w[1]), "at {width}, the parts shown change: {seen:?}");
        let gpu = seen[0].iter().any(|t| t.starts_with("GPU"));
        if width >= fits + 4.0 {
            assert!(gpu, "at {width}, where all fit, the GPU part was left out");
        } else if width <= fits - 4.0 {
            assert!(!gpu, "at {width}, where they do not all fit, the GPU part was kept");
        }
    }
}

/// The scripts part is drawn by what the frame read, not by the server read again (which
/// another thread may have changed meanwhile, a script leaving): here the server, there being
/// none, would say none are connected, and so none shut out.
#[test]
fn the_status_bar_s_scripts_part_is_drawn_by_what_the_frame_read() {
    use egui_kittest::kittest::by;
    let _gpu = gpu_lock();
    let mut h = harness();
    let connected = |h: &Harness<'static, OpalApp>| h.query(by().label_contains("A script is connected")).is_some();
    let let_in = |h: &Harness<'static, OpalApp>| h.query(by().label("Let in")).is_some();
    h.state_mut().pretend_status(1, 0);
    h.run();
    assert!(connected(&h), "not drawn as the frame read it");
    assert!(!let_in(&h), "shown as shut out, which it is not");
    // Read gone, it goes.
    h.state_mut().pretend_status(0, 0);
    h.run();
    assert!(!connected(&h) && !let_in(&h), "still shown");
}

/// The status bar is laid out again in the frame only where its parts grow past the room it
/// gave them: narrower, the next frame does.
#[test]
fn the_status_bar_takes_another_pass_only_where_its_parts_grow() {
    let _gpu = gpu_lock();
    let mut h = harness();
    pasting(&mut h);
    h.state_mut().pin_gpu_memory((212 << 20, 48 << 30));
    h.state_mut().pretend_status(0, 0);
    h.state_mut().state_mut().set_unit(opal_core::units::Unit::Mm);
    h.run();
    let passes = |h: &mut Harness<'static, OpalApp>| {
        let before = h.ctx.cumulative_pass_nr();
        h.step();
        h.ctx.cumulative_pass_nr() - before
    };
    assert_eq!(passes(&mut h), 1, "a frame with nothing changed took another pass");
    // In pixels, the canvas's size is shorter: no other pass.
    h.state_mut().state_mut().set_unit(opal_core::units::Unit::Px);
    assert_eq!(passes(&mut h), 1, "parts grown narrower took another pass");
    // In millimetres again, longer: another pass, so that it does not run over the text.
    h.state_mut().state_mut().set_unit(opal_core::units::Unit::Mm);
    assert_eq!(passes(&mut h), 2, "parts grown wider took no other pass");
}

/// The Presets menu beside a tool's name saves its settings under a name, applies them, and
/// deletes them.
#[test]
fn the_presets_menu_saves_applies_and_deletes_a_tool_s_settings() {
    use crate::settings::Value;
    use crate::tools::ToolId;
    use egui_kittest::kittest::by;
    let _gpu = gpu_lock();
    let mut h = harness();
    h.state_mut().state_mut().set_tool(ToolId::Crop);
    image_of(&mut h, 400, 300);
    let guides = |h: &Harness<'static, OpalApp>| h.state().state().tool_settings[&ToolId::Crop].get("crop.guides");
    let set_guides = |h: &mut Harness<'static, OpalApp>, v: &str| {
        let s = h.state_mut().state_mut().tool_settings.get_mut(&ToolId::Crop).unwrap();
        s.set("crop.guides", Value::Choice(v.to_owned()));
    };
    let saved = |h: &Harness<'static, OpalApp>| {
        h.state().state().presets.get(&ToolId::Crop).map(|p| p.keys().cloned().collect::<Vec<_>>()).unwrap_or_default()
    };
    let menu = |h: &mut Harness<'static, OpalApp>| {
        h.get(on_side(by().label("Presets").role(egui::accesskit::Role::Button), false)).click();
        h.run();
    };
    let naming = |h: &Harness<'static, OpalApp>| h.state().state().preset_name.clone();
    let save_as = |h: &mut Harness<'static, OpalApp>| {
        menu(h);
        h.get_by_label("Save as preset…").click();
        h.run();
    };
    set_guides(&mut h, "grid");
    h.run();
    // Saved under a name typed below the heading, which has the keyboard.
    save_as(&mut h);
    // Enter with no name keeps the keyboard there, for one to be typed.
    h.key_press(egui::Key::Enter);
    h.run();
    assert!(h.query(focused()).is_some(), "Enter with no name let go of the keyboard");
    h.get(focused()).type_text("Gri");
    h.run();
    // Meanwhile another field takes the keyboard, and keeps it, and Esc in it leaves the name.
    h.get(field_showing(400.0, 1e-6, false)).click();
    h.run();
    h.get(focused()).type_text("5");
    h.key_press(egui::Key::Escape);
    h.run();
    assert_eq!(naming(&h).as_deref(), Some("Gri"), "the name took the keyboard, or Esc elsewhere dropped it");
    // Save as… again gives the name back the keyboard, as it was typed.
    save_as(&mut h);
    assert_eq!(naming(&h).as_deref(), Some("Gri"), "Save as… again forgot the name being typed");
    h.get(focused()).type_text("d");
    h.key_press(egui::Key::Enter);
    h.run();
    assert_eq!(saved(&h), ["Grid"]);
    assert!(naming(&h).is_none(), "the name is still being asked for");
    // Esc in the name lets it go.
    save_as(&mut h);
    h.key_press(egui::Key::Escape);
    h.run();
    assert!(naming(&h).is_none(), "Esc did not let the name go");
    // Asked for while something else takes the panel from the next frame on (a paste, its box
    // opened at the end of the frame of the click), the name does not take the keyboard when
    // it shows again later. (The click and the paste in one frame: kittest gives each event
    // queued a frame of its own.)
    menu(&mut h);
    put_on_clipboard(&mut h, pasted(20, 20, [1.0, 0.0, 0.0, 1.0]));
    let at = h.get_by_label("Save as preset…").rect().center();
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let events = [egui::Event::PointerMoved(at), button(true), button(false), egui::Event::Paste(String::new())];
    h.input_mut().events.extend(events);
    h.step();
    h.run();
    assert!(h.state().state().active().unwrap().transform.is_some(), "the paste did not open its box");
    h.key_press(egui::Key::Escape);
    h.run();
    assert_eq!(naming(&h).as_deref(), Some(""), "the name was not shown again");
    assert!(h.query(focused()).is_none(), "the name took the keyboard long after it was asked to");
    // A name typed for one tool is let go when another is chosen.
    h.state_mut().state_mut().set_tool(ToolId::Move);
    h.run();
    h.state_mut().state_mut().set_tool(ToolId::Crop);
    h.run();
    assert!(naming(&h).is_none(), "a name typed for the crop tool was kept for another");
    // Applied from the menu.
    set_guides(&mut h, "none");
    h.run();
    menu(&mut h);
    h.get(by().label("Grid").role(egui::accesskit::Role::Button)).click();
    h.run();
    assert_eq!(guides(&h), Value::Choice("grid".to_owned()), "the preset was not applied");
    // Deleted from its submenu.
    menu(&mut h);
    h.get_by_label("Delete a preset").hover();
    h.run();
    h.get_by_label("Delete Grid").click();
    h.run();
    assert!(saved(&h).is_empty(), "the preset was not deleted");
}

/// A grid of a box's points is one stop for Tab, the point chosen, and the arrow keys move the
/// choice within it.
#[test]
fn a_grid_of_points_is_one_tab_stop_and_the_arrows_move_in_it() {
    use egui_kittest::kittest::by;
    let _gpu = gpu_lock();
    let mut h = harness();
    pasting(&mut h);
    h.state_mut().state_mut().box_numbers = true;
    h.run();
    let focus_on = |h: &Harness<'static, OpalApp>| {
        use egui_kittest::kittest::NodeT;
        h.get(focused()).accesskit_node().label().unwrap_or_default()
    };
    let chosen = |h: &Harness<'static, OpalApp>| h.state().state().prefs.choice("transform.reference");
    let units = || on_side(by().label("Units").role(egui::accesskit::Role::ComboBox), true);
    // From the units, Tab reaches the point chosen, then the next menu.
    h.get(units()).focus();
    h.run();
    h.key_press(egui::Key::Tab);
    h.run();
    assert_eq!(focus_on(&h), "Top left");
    h.key_press(egui::Key::ArrowRight);
    h.run();
    h.key_press(egui::Key::ArrowDown);
    h.run();
    assert_eq!(chosen(&h), "center", "the arrows did not move the choice");
    assert_eq!(focus_on(&h), "Center", "the focus did not move with the choice");
    // At the grid's edge an arrow moves nothing, and the focus stays (egui, given the arrow,
    // would move it on to what lies that way: the units).
    h.key_press(egui::Key::ArrowUp);
    h.run();
    h.key_press(egui::Key::ArrowUp);
    h.run();
    assert_eq!(chosen(&h), "top");
    assert_eq!(focus_on(&h), "Top", "the focus left the grid at its edge");
    h.key_press(egui::Key::Tab);
    h.run();
    assert_eq!(focus_on(&h), "From", "Tab went through the other points");
    // Shift+Tab goes back to the point chosen, past the others.
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Tab);
    h.run();
    assert_eq!(focus_on(&h), "Top", "Shift+Tab went through the other points");
    // Arrows pressed between two frames each count (kittest gives each queued event a frame of
    // its own, so these go into one frame's input directly).
    for _ in 0..2 {
        let down = |pressed| egui::Event::Key {
            key: egui::Key::ArrowDown,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        h.input_mut().events.extend([down(true), down(false)]);
    }
    h.step();
    h.run();
    assert_eq!(chosen(&h), "bottom", "an arrow was lost");
    assert_eq!(focus_on(&h), "Bottom");
    // From the units, the down arrow reaches the point chosen, not one chosen before it.
    h.get(units()).focus();
    h.run();
    h.key_press(egui::Key::ArrowDown);
    h.run();
    assert_eq!(focus_on(&h), "Bottom", "the down arrow did not reach the point chosen");
}

/// A unit chosen in the menu below a box's numbers shows them in it in the same frame.
#[test]
fn a_unit_chosen_below_a_box_s_numbers_shows_them_in_it_at_once() {
    use egui_kittest::kittest::by;
    use opal_core::units::Unit;
    let _gpu = gpu_lock();
    let mut h = harness();
    pasting(&mut h);
    h.state_mut().state_mut().box_numbers = true;
    h.run();
    let x = box_params(&h).x;
    h.get(on_side(by().label("Units").role(egui::accesskit::Role::ComboBox), true)).click();
    h.run();
    h.get_by_label(Unit::In.name()).click();
    h.step();
    let shown = Unit::In.from_px(x, 100.0);
    assert!(
        h.query(box_field(shown, 1e-6)).is_some(),
        "x is not shown in inches ({shown}) in the frame they were chosen"
    );
}

#[test]
fn a_box_s_field_takes_an_expression_and_says_what_it_comes_to() {
    use egui_kittest::kittest::by;
    let _gpu = gpu_lock();
    let mut h = harness();
    pasting(&mut h);
    // Typed into the numbers by the box (the tool settings show the same).
    h.state_mut().state_mut().box_numbers = true;
    h.run();
    let x = |h: &Harness<'static, OpalApp>| box_params(h).x;
    let at = x(&h);
    // The From label is level with the menu it names, and the grid of points the position
    // counts from lies between the menu and what follows the fields.
    let by_the_box = |b| h.get(on_side(b, true)).rect();
    let label = by_the_box(by().label("From").role(egui::accesskit::Role::Label));
    let menu = by_the_box(by().label("From").role(egui::accesskit::Role::ComboBox));
    assert!(
        (label.center().y - menu.center().y).abs() < 1.0,
        "the From label is not beside its menu: {label:?} {menu:?}"
    );
    let (first, last) = (by_the_box(by().label("From: Top left")), by_the_box(by().label("From: Bottom left")));
    let below = by_the_box(by().label("Fit into a rectangle"));
    assert!(
        menu.bottom() <= first.top() && last.bottom() <= below.top(),
        "the From grid reaches into the rows beside it: {menu:?} {first:?} {last:?} {below:?}"
    );
    // A layer's edge, the box's own width, and a bare number in the field's unit (pixels):
    // shown whole, with what it comes to, below the field (which may be too narrow to show
    // it), while what is beside the field stays where it was.
    let beside = h.get(box_field(box_params(&h).y, 1e-6)).rect();
    type_into(&mut h, box_field(at, 1e-6), "Background.right - width - 10");
    let whole = h.query_all(by().label_contains("Background.right - width - 10  = 340 px")).count();
    assert_eq!(whole, 1, "what is typed, and what it comes to, are not shown whole");
    assert_eq!(h.get(box_field(box_params(&h).y, 1e-6)).rect(), beside, "what is beside the field moved");
    h.key_press(egui::Key::Enter);
    h.run();
    assert_eq!(x(&h), 340.0);
    // What cannot be worked out says why, and where; Enter keeps it, to be put right.
    type_into(&mut h, box_field(340.0, 1e-6), "Bakground.right");
    assert!(h.query_all(by().label_contains("no layer called “Bakground”")).count() >= 1, "the error is not shown");
    h.key_press(egui::Key::Enter);
    h.run();
    h.run();
    assert_eq!(x(&h), 340.0, "what could not be worked out moved the box");
    let kept = h.query_all(by().predicate(|n| n.value().as_deref() == Some("Bakground.right"))).count();
    assert!(kept >= 1, "the text was not kept to be put right");
    // Escape leaves the field as it was, and the box open (egui lets go of a field's focus
    // on Esc before the frame's keys are seen).
    h.key_press(egui::Key::Escape);
    h.run();
    let open = |h: &Harness<'static, OpalApp>| h.state().state().active().unwrap().transform.is_some();
    assert!(open(&h), "Esc in a field cancelled the box");
    assert_eq!(x(&h), 340.0);
    // Up brings back what was entered.
    h.get(box_field(340.0, 1e-6)).click();
    h.run();
    h.key_press(egui::Key::ArrowUp);
    h.run();
    let back = h.query_all(by().predicate(|n| n.value().as_deref() == Some("Background.right - width - 10"))).count();
    assert!(back >= 1, "Up did not bring back the last entry");
    h.key_press(egui::Key::Escape);
    h.run();
    // What was entered is kept with the settings, for the next session.
    // (A length's apart in each unit, as a bare number in it is in the unit.)
    let kept = h.state().state().field_entries.get("box.x.px").cloned().unwrap_or_default();
    assert_eq!(kept.first().map(String::as_str), Some("Background.right - width - 10"), "{kept:?}");
}

#[test]
fn a_field_brings_back_what_was_entered_in_an_earlier_session() {
    use egui_kittest::kittest::by;
    let _gpu = gpu_lock();
    let mut h = harness();
    // As loaded from the settings at start.
    let earlier = std::collections::BTreeMap::from([
        ("box.x.px".to_owned(), vec!["canvas.width / 3".to_owned()]),
        ("box.x.in".to_owned(), vec!["2".to_owned()]),
    ]);
    crate::number_field::load_entries(&h.ctx, earlier);
    pasting(&mut h);
    h.state_mut().state_mut().box_numbers = true;
    h.run();
    let at = box_params(&h).x;
    h.get(box_field(at, 1e-6)).click();
    h.run();
    h.key_press(egui::Key::ArrowUp);
    h.run();
    let back = h.query_all(by().predicate(|n| n.value().as_deref() == Some("canvas.width / 3"))).count();
    assert!(back >= 1, "the earlier session's entry did not come back");
    h.key_press(egui::Key::Escape);
    h.run();
}

#[test]
fn a_field_left_untouched_changes_nothing_and_tab_enters_what_was_typed() {
    let _gpu = gpu_lock();
    let mut h = harness();
    let st = h.state_mut().state_mut();
    st.create_image(&crate::state::NewImage {
        width: 400,
        height: 300,
        background: crate::state::Background::White,
        resolution: 300.0,
    });
    st.set_unit(opal_core::units::Unit::In);
    st.box_numbers = true;
    put_on_clipboard(&mut h, pasted(50, 40, [1.0, 0.0, 0.0, 1.0]));
    h.event(egui::Event::Paste(String::new()));
    h.run();
    // At x = 100 px, which shows rounded: 0.333 in.
    {
        let d = h.state_mut().state_mut().active_mut().unwrap();
        let p = d.transform.as_ref().unwrap().params();
        let m = d.transform.as_ref().unwrap().current.then(opal_core::geom::Affine::translate(100.0 - p.x, 0.0));
        d.set_box(m, None);
    }
    h.run();
    let x = |h: &Harness<'static, OpalApp>| box_params(h).x;
    let field = |v: f64| box_field(v, 1e-3);
    // Clicked, and left by Tab with nothing typed: still 100 px, not 0.333 in.
    h.get(field(1.0 / 3.0)).click();
    h.run();
    h.key_press(egui::Key::Tab);
    h.run();
    assert_eq!(x(&h), 100.0, "leaving the field moved the box to what it showed");
    h.key_press(egui::Key::Escape);
    h.run();
    // So too by Enter.
    h.get(field(1.0 / 3.0)).click();
    h.run();
    h.key_press(egui::Key::Enter);
    h.run();
    assert_eq!(x(&h), 100.0, "Enter on what was not typed moved the box");
    // Typed, and left by Tab: what was typed is entered.
    type_into(&mut h, field(1.0 / 3.0), "1");
    h.key_press(egui::Key::Tab);
    h.run();
    assert_eq!(x(&h), 300.0, "Tab threw away what was typed");
}

#[test]
fn an_expression_typed_into_the_box_places_the_layer_so_again() {
    use egui_kittest::kittest::by;
    let _gpu = gpu_lock();
    let mut h = harness();
    new_image(&mut h, 400, 300, 100.0);
    h.state_mut().state_mut().box_numbers = true;
    let x = |h: &Harness<'static, OpalApp>| box_params(h).x;
    // Pasted, its x typed as what names the canvas, entered, and the box applied.
    let paste = |h: &mut Harness<'static, OpalApp>, drag: bool| {
        put_on_clipboard(h, pasted(50, 40, [1.0, 0.0, 0.0, 1.0]));
        h.event(egui::Event::Paste(String::new()));
        h.run();
        let at = x(h);
        type_into(h, box_field(at, 1e-6), "canvas.width - 60");
        h.key_press(egui::Key::Enter);
        h.run();
        assert_eq!(x(h), 340.0);
        if drag {
            let d = h.state_mut().state_mut().active_mut().unwrap();
            let m = d.transform.as_ref().unwrap().current.then(opal_core::geom::Affine::translate(-5.0, 0.0));
            d.set_box(m, None);
        }
        h.key_press(egui::Key::Enter);
        h.run();
        let d = h.state().state().active().unwrap();
        d.active_layer().unwrap()
    };
    let placed =
        |h: &Harness<'static, OpalApp>, l| h.state().state().active().unwrap().doc().layer(l).unwrap().placed.clone();
    let kept = paste(&mut h, false);
    let p = placed(&h, kept).expect("what was typed was not kept");
    assert_eq!((p.numbers.len(), p.numbers[0].expr.as_str()), (1, "canvas.width - 60"));
    // Dragged after it was typed, it no longer says where the box is: not kept.
    let dragged = paste(&mut h, true);
    assert_eq!(placed(&h, dragged), None, "what a drag undid was kept");
    // The canvas wider, the box opened on the first layer says how it was placed, and its
    // Re-apply puts it where that says now.
    {
        let st = h.state_mut().state_mut();
        let resized = st.call(
            "resize_canvas",
            &serde_json::json!({ "width": 500, "anchor": "top-left" }).as_object().unwrap().clone(),
            &egui::Context::default(),
        );
        assert!(matches!(resized, crate::api::Reply::Now(Ok(_))), "{resized:?}");
        let d = st.active_mut().unwrap();
        d.select_only(kept);
        d.make_active(kept);
    }
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::T);
    h.run();
    assert!(h.query_all(by().label("x = canvas.width - 60")).count() >= 1, "the box does not say how it was placed");
    let again = format!("{}  Re-apply", crate::icons::REAPPLY);
    h.get(on_side(by().label(&again), true)).click();
    h.run();
    assert_eq!(x(&h), 440.0, "Re-apply did not work it out again");
}

/// The README's picture, `docs/screenshot.png`: an image of a few layers, and the box placing
/// a pasted badge by an expression being typed into the tool settings. Made again with
/// `OPAL_WRITE_SCREENSHOT=1 cargo test -p opal-app --bin opal-studio the_readme_screenshot -- --ignored`.
#[test]
#[ignore = "makes the README's screenshot (OPAL_WRITE_SCREENSHOT=1, --ignored)"]
fn the_readme_screenshot() {
    use egui_kittest::kittest::by;
    use opal_core::document::{Document, IdGen, Layer, Position};
    use opal_core::raster::RasterImage;
    let _gpu = gpu_lock();
    // Tall enough for the whole of the tool settings.
    let mut h = harness_of([1400.0, 1100.0]);
    // Colors in 8-bit sRGB, straight alpha.
    let mix = |a: [f32; 3], b: [f32; 3], t: f32| [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t.clamp(0.0, 1.0));
    let byte = |c: [f32; 3], a: f32| {
        let [r, g, b] = c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
        [r, g, b, (a.clamp(0.0, 1.0) * 255.0).round() as u8]
    };
    let hex = |v: u32| [(v >> 16) as f32 / 255.0, ((v >> 8) & 255) as f32 / 255.0, (v & 255) as f32 / 255.0];
    // A sky at dusk, with the sun's glow low on the right.
    let (w, ht) = (1200u32, 800u32);
    let stops = [hex(0x10163a), hex(0x2f3a86), hex(0x8a5a9e), hex(0xe2836a), hex(0xf6c98a)];
    let sky: Vec<[u8; 4]> = (0..w * ht)
        .map(|i| {
            let (x, y) = ((i % w) as f32, (i / w) as f32);
            let t = y / ht as f32 * (stops.len() - 1) as f32;
            let k = (t.floor() as usize).min(stops.len() - 2);
            let base = mix(stops[k], stops[k + 1], t - k as f32);
            let glow = (-((x - 930.0).powi(2) + (y - 640.0).powi(2)) / (2.0 * 170f32.powi(2))).exp();
            byte(mix(base, hex(0xffe3a3), glow * 0.85), 1.0)
        })
        .collect();
    // An opal: a disc whose color turns with the angle and the distance from its middle,
    // a light caught at its upper left.
    let r = 180.0f32;
    let side = (2.0 * r + 4.0) as u32;
    let opal: Vec<[u8; 4]> = (0..side * side)
        .map(|i| {
            let (dx, dy) = ((i % side) as f32 - side as f32 / 2.0, (i / side) as f32 - side as f32 / 2.0);
            let d = dx.hypot(dy);
            let hue = (dy.atan2(dx) / std::f32::consts::TAU + d / r * 0.45).rem_euclid(1.0);
            let ring = [0.0, 1.0 / 3.0, 2.0 / 3.0].map(|o| 0.5 + 0.5 * ((hue + o) * std::f32::consts::TAU).cos());
            let pastel = mix(ring, [1.0, 1.0, 1.0], 0.45 - d / r * 0.15);
            let shine = (-((dx + r * 0.38).powi(2) + (dy + r * 0.38).powi(2)) / (2.0 * (r * 0.28).powi(2))).exp();
            byte(mix(pastel, [1.0, 1.0, 1.0], shine * 0.7), r + 0.5 - d)
        })
        .collect();
    let mut ids = IdGen::default();
    let mut doc = Document::new(w, ht);
    doc.resolution = 300.0;
    let top = |index| Position { parent: None, index };
    let sky = Layer::new_raster(ids.layer(), ids.object(), "Sky", RasterImage::from_srgb8(w, ht, &sky));
    doc.insert(top(0), sky).unwrap();
    let disc = opal_core::document::PixelObject {
        image: RasterImage::from_srgb8(side, side, &opal).into(),
        offset: [250, 210],
    };
    let disc = opal_core::document::Object { id: ids.object(), kind: opal_core::document::ObjectKind::Pixels(disc) };
    doc.insert(top(1), Layer::with_objects(ids.layer(), "Opal", vec![disc])).unwrap();
    {
        let st = h.state_mut().state_mut();
        st.new_untitled(doc, ids, "New image");
        st.set_unit(opal_core::units::Unit::Mm);
    }
    // The tool settings given room for all of them, and the layers what they need.
    h.state_mut().set_pane_share(crate::app::Pane::ToolSettings, 0.72);
    h.state_mut().set_pane_share(crate::app::Pane::Layers, 0.28);
    // A gold badge, pasted.
    let (bw, bh, round) = (300u32, 120u32, 26.0f32);
    let badge: Vec<[u8; 4]> = (0..bw * bh)
        .map(|i| {
            let (x, y) = ((i % bw) as f32 + 0.5, (i / bw) as f32 + 0.5);
            let (qx, qy) = (
                (x - bw as f32 / 2.0).abs() - (bw as f32 / 2.0 - round),
                (y - bh as f32 / 2.0).abs() - (bh as f32 / 2.0 - round),
            );
            let edge = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - round;
            let gold = mix(hex(0xffe08a), hex(0xd98e2b), y / bh as f32);
            let rim = mix(gold, hex(0x8a4b12), ((edge + 6.0) / 6.0).clamp(0.0, 1.0) * 0.8);
            byte(rim, 0.5 - edge)
        })
        .collect();
    let image = RasterImage::from_srgb8(bw, bh, &badge);
    put_on_clipboard(
        &mut h,
        crate::clipboard::Pasted { width: bw, height: bh, image, from: None, resolution: None, warnings: Vec::new() },
    );
    h.event(egui::Event::Paste(String::new()));
    h.run();
    h.run();
    // Its x being typed as what names the opal: what it comes to shows below.
    let at = box_params(&h).x;
    let shown = opal_core::units::Unit::Mm.from_px(at, 300.0);
    type_into(&mut h, field_showing(shown, 1e-2, false), "Opal.right + 5mm");
    // The pointer away, so that it hides none of it.
    h.event(egui::Event::PointerGone);
    for _ in 0..4 {
        h.run();
    }
    // It shows what the expression comes to, not an error.
    assert!(h.query_all(by().label_contains("= 56.9 mm")).count() >= 1, "the preview does not show the value");
    let img = h.render().expect("the app renders");
    let path = match std::env::var_os("OPAL_WRITE_SCREENSHOT") {
        Some(_) => std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/screenshot.png"),
        None => std::env::temp_dir().join("opal-ui-readme.png"),
    };
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    img.save(&path).unwrap();
}

#[test]
fn the_box_s_buttons_stay_in_the_view_however_many_it_has() {
    let _gpu = gpu_lock();
    let mut h = harness();
    let st = h.state_mut().state_mut();
    st.create_image(&crate::state::NewImage {
        width: 400,
        height: 300,
        background: crate::state::Background::White,
        resolution: 100.0,
    });
    // A file placed whose resolution is not the image's has every button there is.
    let path = std::env::temp_dir().join(format!("opal-ui-place-edge-{}.jpg", std::process::id()));
    crate::paste::testing::white_jpeg(&path, 90, 60, Some([300, 300]));
    st.place_file(&path, None, true).unwrap();
    std::fs::remove_file(&path).ok();
    h.run();
    // Moved past the view's right edge: its buttons stop at the edge, all of them inside.
    let d = h.state_mut().state_mut().active_mut().unwrap();
    let right = d.view.screen_rect.right();
    let m = d.transform.as_ref().unwrap().current;
    d.set_box(m.then(opal_core::geom::Affine::translate(4000.0, 0.0)), None);
    h.run();
    h.run();
    let last = h.get_by_label(&crate::icons::INTO_LAYER.to_string()).rect();
    let first = h.get_by_label(&crate::icons::CHECK.to_string()).rect();
    assert!(last.right() <= right, "the last button ends at {} past the view's edge at {right}", last.right());
    assert!(right - last.right() < 40.0, "the buttons stop short of the edge, at {}", last.right());
    assert!(first.left() < last.left());
}

#[test]
fn the_print_size_button_says_why_it_cannot_switch() {
    let _gpu = gpu_lock();
    let mut h = harness();
    let st = h.state_mut().state_mut();
    st.prefs.set("transform.fit_large", crate::settings::Value::Bool(false));
    st.prefs.set("transform.place_print_size", crate::settings::Value::Bool(false));
    st.create_image(&crate::state::NewImage {
        width: 400,
        height: 300,
        background: crate::state::Background::White,
        resolution: opal_core::units::MAX_PPI,
    });
    // At 1 ppi, its print size here is a million times its pixels: as far as a box may
    // scale, so enlarged first, the switch would take it past that.
    let path = std::env::temp_dir().join(format!("opal-ui-place-far-{}.jpg", std::process::id()));
    crate::paste::testing::white_jpeg(&path, 2, 2, Some([1, 1]));
    st.place_file(&path, None, false).unwrap();
    std::fs::remove_file(&path).ok();
    let d = st.active_mut().unwrap();
    let s = d.transform.as_ref().unwrap();
    let [cx, cy] = s.current.apply(s.frame.center());
    let twice = s.current.then(opal_core::geom::Affine::scale(2.0, 2.0).about(cx, cy));
    d.set_box(twice, None);
    h.run();
    h.get_by_label(&crate::icons::PRINT_SIZE.to_string()).click();
    h.run();
    let st = h.state().state();
    assert_eq!(st.active().unwrap().transform.as_ref().unwrap().params().width, 4.0, "it switched");
    assert!(st.notice.as_ref().is_some_and(|n| n.0.contains("cannot be switched")), "{:?}", st.notice);
    assert!(!st.prefs.bool("transform.place_print_size"), "a switch refused was remembered");
}

#[test]
fn the_resolution_in_the_status_bar_opens_resize_image() {
    let _gpu = gpu_lock();
    let mut h = harness();
    new_image(&mut h, 40, 30, 150.0);
    h.run();
    h.get_by_label("150 ppi").click();
    h.run();
    let st = h.state().state();
    assert!(st.resize_image.as_ref().is_some_and(|r| r.resolution == 150.0), "Resize Image did not open");
}

#[test]
fn ribbon_tabs_switch_and_commands_run() {
    let _gpu = gpu_lock();
    let mut h = harness();
    h.state_mut().state_mut().create_image(&crate::state::NewImage {
        width: 64,
        height: 64,
        background: crate::state::Background::Transparent,
        resolution: opal_core::units::DEFAULT_PPI,
    });
    h.run();
    h.get_by_label("Layer").click();
    h.run();
    h.get_by_label("New Layer").click();
    h.run();
    assert_eq!(h.state().state().active().unwrap().doc().layers.len(), 2);
    // Ctrl+Z takes it back.
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    h.run();
    assert_eq!(h.state().state().active().unwrap().doc().layers.len(), 1);
}

#[test]
fn command_search_finds_and_runs_a_command() {
    let _gpu = gpu_lock();
    let mut h = harness();
    h.run();
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::K);
    h.run();
    assert!(h.get_by_role(egui::accesskit::Role::TextInput).is_focused(), "Ctrl+K focuses the search box");
    h.get_by_role(egui::accesskit::Role::TextInput).type_text("prefer");
    h.run();
    assert_eq!(h.state().state().ribbon.search, "prefer", "the text went into the search box");
    h.key_press(egui::Key::Enter);
    h.run();
    assert!(h.state().state().prefs_open);
}

#[test]
fn a_white_image_draws_white_on_the_backdrop() {
    let _gpu = gpu_lock();
    let mut h = harness();
    new_image(&mut h, 400, 300, opal_core::units::DEFAULT_PPI);
    h.run();
    let img = render(&mut h, "white-canvas");
    // The middle of the canvas is the white image; just outside it is the backdrop.
    assert_eq!(pixel_at_canvas(&h, &img, 200.0, 150.0), [255, 255, 255]);
    let backdrop = h.state().state().palette.backdrop;
    assert_eq!(pixel_at_canvas(&h, &img, -10.0, 150.0), [backdrop.r(), backdrop.g(), backdrop.b()]);
}

#[test]
fn a_transparent_image_shows_the_checkerboard() {
    let _gpu = gpu_lock();
    let mut h = harness();
    h.state_mut().state_mut().create_image(&crate::state::NewImage {
        width: 400,
        height: 300,
        background: crate::state::Background::Transparent,
        resolution: opal_core::units::DEFAULT_PPI,
    });
    h.run();
    let img = render(&mut h, "transparent-canvas");
    // 8-pixel squares at 100%: white at (2, 2), the second color at (10, 2).
    assert_eq!(pixel_at_canvas(&h, &img, 2.0, 2.0), [255, 255, 255]);
    assert_eq!(pixel_at_canvas(&h, &img, 10.0, 2.0), [204, 204, 204]);
}

#[test]
fn content_beyond_the_canvas_is_dimmed_and_can_be_hidden() {
    let _gpu = gpu_lock();
    let mut h = harness();
    {
        let st = h.state_mut().state_mut();
        st.create_image(&crate::state::NewImage {
            width: 400,
            height: 300,
            background: crate::state::Background::Transparent,
            resolution: opal_core::units::DEFAULT_PPI,
        });
        // A white square left of the canvas, as a layer that extends past its edge would have.
        let d = st.active_mut().unwrap();
        let layer = d.active_layer().unwrap();
        d.apply::<()>("paint", |doc, ids| {
            let mut img = opal_core::raster::RasterImage::new();
            for y in 100..150 {
                for x in -60..-10 {
                    img.set_pixel(x, y, opal_core::tile::pixel_from_f32([1.0; 4]));
                }
            }
            *doc.layer_mut(layer).unwrap() =
                opal_core::document::Layer::new_raster(layer, ids.object(), "Background", img);
            Ok(())
        })
        .unwrap();
    }
    h.run();
    let img = render(&mut h, "off-canvas");
    // 35% of white over the backdrop, mixed in linear light.
    let backdrop = h.state().state().palette.backdrop;
    let b = opal_color::srgb_decode(f64::from(backdrop.r()) / 255.0);
    let want = (opal_color::srgb_encode(b * 0.65 + 0.35) * 255.0).round() as i32;
    let got = pixel_at_canvas(&h, &img, -35.0, 125.0);
    assert!((i32::from(got[0]) - want).abs() <= 2, "got {got:?}, want about {want}");
    // Turned off, it shows nothing.
    h.state_mut().state_mut().queue.push(Cmd::ToggleOffCanvas);
    h.run();
    let img = render(&mut h, "off-canvas-hidden");
    assert_eq!(pixel_at_canvas(&h, &img, -35.0, 125.0), [backdrop.r(), backdrop.g(), backdrop.b()]);
}

/// The rendered color at canvas point `(x, y)` of the active document.
fn pixel_at_canvas(h: &Harness<'static, OpalApp>, img: &image::RgbaImage, x: f32, y: f32) -> [u8; 3] {
    let d = h.state().state().active().unwrap();
    let v = d.view;
    let ppp = img.width() as f32 / 1400.0;
    let px = v.screen_rect.min.to_vec2() * ppp + v.to_screen(egui::vec2(x, y));
    let p = img.get_pixel(px.x as u32, px.y as u32);
    [p[0], p[1], p[2]]
}

#[test]
fn render_the_main_states() {
    let _gpu = gpu_lock();
    // Not checks of their own (the accessibility check runs on every frame), but renderings
    // to look at: they are saved in the temporary folder as opal-ui-*.png.
    let mut h = harness();
    h.run();
    render(&mut h, "start");
    {
        let st = h.state_mut().state_mut();
        st.create_image(&crate::state::NewImage {
            width: 1200,
            height: 800,
            background: crate::state::Background::White,
            resolution: opal_core::units::DEFAULT_PPI,
        });
        st.queue.push(Cmd::NewLayer);
        st.queue.push(Cmd::NewGroup);
        st.ribbon.tab = 2;
    }
    h.run();
    render(&mut h, "layer-tab");
    h.state_mut().state_mut().ribbon.keytips = crate::state::Keytips::Buttons(2);
    h.run();
    render(&mut h, "keytips");
    h.state_mut().state_mut().ribbon.keytips = crate::state::Keytips::Off;
    h.state_mut().state_mut().queue.push(Cmd::ToggleTheme);
    h.run();
    render(&mut h, "light");
    h.state_mut().state_mut().queue.push(Cmd::ToggleTheme);
    h.state_mut().state_mut().prefs_open = true;
    h.run();
    render(&mut h, "preferences");
    h.state_mut().state_mut().prefs_open = false;
    h.state_mut().state_mut().help = Some(crate::state::HelpTopic::Group(2, 0));
    h.run();
    render(&mut h, "help");
    h.state_mut().state_mut().help = None;
    h.set_size(egui::vec2(760.0, 600.0));
    h.run();
    render(&mut h, "narrow");
}

#[test]
fn keytips_open_a_tab_and_run_a_command() {
    let _gpu = gpu_lock();
    let mut h = harness();
    new_image(&mut h, 32, 32, opal_core::units::DEFAULT_PPI);
    h.run();
    // With the keytips shown, H picks the Home tab, and N runs its New Layer.
    h.state_mut().state_mut().ribbon.keytips = crate::state::Keytips::Tabs;
    h.run();
    h.key_press(egui::Key::H);
    h.run();
    assert_eq!(h.state().state().ribbon.keytips, crate::state::Keytips::Buttons(1));
    assert_eq!(h.state().state().ribbon.tab, 1);
    h.key_press(egui::Key::N);
    h.run();
    assert_eq!(h.state().state().ribbon.keytips, crate::state::Keytips::Off);
    assert_eq!(h.state().state().active().unwrap().doc().layers.len(), 2);
}

// ---- Regression tests for the milestone 0 review ----------------------------------------

fn with_image(h: &mut Harness<'static, OpalApp>, background: crate::state::Background) {
    h.state_mut().state_mut().create_image(&crate::state::NewImage {
        width: 64,
        height: 64,
        background,
        resolution: opal_core::units::DEFAULT_PPI,
    });
    h.run();
}

#[test]
fn shift_shortcuts_run_their_own_command_not_their_ctrl_twin() {
    let _gpu = gpu_lock();
    let mut h = harness();
    with_image(&mut h, crate::state::Background::White);
    // Ctrl+Shift+N is New Layer; Ctrl+N (New image) must not take it.
    h.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::N);
    h.run();
    assert!(h.state().state().new_image.is_none(), "Ctrl+Shift+N opened the New Image dialog");
    assert_eq!(h.state().state().active().unwrap().doc().layers.len(), 2);
    // Ctrl+Shift+Z is Redo; Ctrl+Z (Undo) must not take it.
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    h.run();
    h.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z);
    h.run();
    assert_eq!(h.state().state().active().unwrap().doc().layers.len(), 2, "Ctrl+Shift+Z did not redo");
}

#[test]
fn clicking_a_layer_name_selects_the_layer() {
    let _gpu = gpu_lock();
    let mut h = harness();
    with_image(&mut h, crate::state::Background::White);
    h.state_mut().state_mut().queue.push(Cmd::NewLayer);
    h.run();
    let bottom = h.state().state().active().unwrap().doc().layers[0].id;
    assert_ne!(h.state().state().active().unwrap().active_layer(), Some(bottom));
    h.get_by_label("Background").click();
    h.run();
    assert_eq!(h.state().state().active().unwrap().active_layer(), Some(bottom));
}

#[test]
fn single_line_ribbon_buttons_do_not_overlap() {
    let _gpu = gpu_lock();
    let mut h = harness();
    h.state_mut().state_mut().queue.push(Cmd::ToggleRibbon);
    h.state_mut().state_mut().ribbon.tab = 0;
    h.run();
    let names = ["New", "Open", "Save", "Save As", "Export", "Close", "Preferences", "Exit"];
    let rects: Vec<egui::Rect> = names.iter().map(|n| h.get_by_label(n).rect()).collect();
    for i in 0..names.len() {
        for j in i + 1..names.len() {
            // Touching edges are fine; sharing area is not.
            let overlap = rects[i].intersect(rects[j]);
            assert!(overlap.width() <= 0.5 || overlap.height() <= 0.5, "{} and {} overlap", names[i], names[j]);
        }
    }
}

#[test]
fn the_shown_tab_is_the_active_document_after_closing_one() {
    let _gpu = gpu_lock();
    let mut h = harness();
    for _ in 0..3 {
        with_image(&mut h, crate::state::Background::White);
    }
    h.state_mut().state_mut().queue.push(Cmd::Close);
    h.run();
    let active = h.state().state().active_doc.expect("a document is still open");
    assert_eq!(h.state().shown_docs(), vec![active], "the panels act on a document that is not on screen");
}

#[test]
fn closing_the_last_image_keeps_the_panel_layout() {
    let _gpu = gpu_lock();
    let mut h = harness();
    with_image(&mut h, crate::state::Background::White);
    // Make the layout differ from the default: widen the documents' share.
    let shares_before = h.state_mut().set_root_share(0.6);
    h.state_mut().state_mut().queue.push(Cmd::Close);
    h.run();
    assert_eq!(h.state().root_share(), shares_before, "closing the last image reset the panels");
    assert!(h.state().shows_welcome());
}

#[test]
fn a_typed_opacity_is_one_history_step() {
    let _gpu = gpu_lock();
    let mut h = harness();
    with_image(&mut h, crate::state::Background::White);
    let states_before = h.state().state().active().unwrap().history.nodes().count();
    let opacity_field = |n: &egui_kittest::kittest::AccessKitNode<'_>| {
        n.role() == egui::accesskit::Role::SpinButton && n.label().as_deref() == Some("Opacity")
    };
    h.get_by(opacity_field).click();
    h.run();
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    // One key at a time, with frames between, as typing happens.
    h.get_by(opacity_field).type_text("7");
    h.run();
    h.get_by(opacity_field).type_text("5");
    h.run();
    h.key_press(egui::Key::Enter);
    h.run();
    let d = h.state().state().active().unwrap();
    assert_eq!(d.history.nodes().count(), states_before + 1, "typing 75 made more than one history state");
    assert!((d.doc().layers[0].opacity - 0.75).abs() < 1e-6, "{}", d.doc().layers[0].opacity);
}

#[test]
fn a_changing_pending_edit_asks_for_a_repaint() {
    let _gpu = gpu_lock();
    let mut h = harness();
    with_image(&mut h, crate::state::Background::White);
    let layer = h.state().state().active().unwrap().active_layer().unwrap();
    let set = |h: &mut Harness<'static, OpalApp>, op: f32| {
        h.state_mut().state_mut().active_mut().unwrap().edit_pending("Change opacity", |d| {
            d.layer_mut(layer).unwrap().opacity = op;
        });
    };
    set(&mut h, 0.9);
    let first = h.state().signature();
    set(&mut h, 0.5);
    assert_ne!(h.state().signature(), first, "the second step of a drag would not be shown");
}

// ---- Regression tests for the second (Fable) review ---------------------------------------

#[test]
fn the_status_bar_shows_the_pointer_s_canvas_position() {
    let _gpu = gpu_lock();
    let mut h = harness();
    with_image(&mut h, crate::state::Background::White);
    let v = h.state().state().active().unwrap().view;
    let ppp = h.ctx.pixels_per_point();
    let at = v.screen_rect.min + v.to_screen(egui::vec2(10.5, 20.5)) / ppp;
    h.hover_at(at);
    h.run();
    h.hover_at(at);
    h.run();
    assert!(h.query_by_label("10, 20").is_some(), "the status bar does not show the position");
}

#[test]
fn shortcuts_wait_while_a_dialog_is_open() {
    let _gpu = gpu_lock();
    let mut h = harness();
    with_image(&mut h, crate::state::Background::White);
    h.state_mut().state_mut().queue.push(Cmd::New);
    h.run();
    h.state_mut().state_mut().new_image.as_mut().unwrap().width = 777;
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::N);
    h.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::N);
    h.run();
    let st = h.state().state();
    assert_eq!(st.new_image.as_ref().map(|n| n.width), Some(777), "Ctrl+N reset the dialog");
    assert_eq!(st.active().unwrap().doc().layers.len(), 1, "Ctrl+Shift+N changed the image behind the dialog");
}

// ---- Milestone 1: paste, transform, move, crop, merge ------------------------------------

/// A `w × h` image of one straight-alpha color, as the clipboard would hand it over.
fn pasted(w: u32, h: u32, rgba: [f32; 4]) -> crate::clipboard::Pasted {
    let px = vec![opal_core::tile::pixel_from_f32(rgba); (w * h) as usize];
    crate::clipboard::Pasted {
        width: w,
        height: h,
        image: opal_core::raster::RasterImage::from_pixels(w, h, &px),
        from: None,
        resolution: None,
        warnings: Vec::new(),
    }
}

/// Opens the box on a 50 × 40 red paste in a new white image, 400 × 300 at 100 ppi (for the
/// next run).
fn pasting(h: &mut Harness<'static, OpalApp>) {
    new_image(h, 400, 300, 100.0);
    put_on_clipboard(h, pasted(50, 40, [1.0, 0.0, 0.0, 1.0]));
    h.event(egui::Event::Paste(String::new()));
}

/// The transform box's numbers (in pixels, of its top-left corner from the canvas's).
fn box_params(h: &Harness<'static, OpalApp>) -> crate::transform::Params {
    h.state().state().active().expect("an image is open").transform.as_ref().expect("the box is open").params()
}

/// Where, across the window, the numbers by the box end and the side panels begin (which
/// take the right 22% of the window: from 1092 in a harness 1400 points wide, as all are
/// that use this; in a narrower one they begin further left).
const PANELS_LEFT: f64 = 1000.0;

/// What `b` finds among the numbers by the box (left of the side panels) if `by_the_box`,
/// else in the side panels (where the tool settings show the same). `b` is not to have a
/// predicate of its own: this one takes its place.
fn on_side(b: egui_kittest::kittest::By<'_>, by_the_box: bool) -> egui_kittest::kittest::By<'_> {
    b.predicate(move |n| n.bounding_box().is_some_and(|r| (r.x0 < PANELS_LEFT) == by_the_box))
}

/// The field that shows `v`, to within `within`: of the numbers by the box (left of the side
/// panels), or of the tool settings (which show the same), as `by_the_box` says.
fn field_showing(v: f64, within: f64, by_the_box: bool) -> egui_kittest::kittest::By<'static> {
    egui_kittest::kittest::by().predicate(move |n| {
        n.role() == egui::accesskit::Role::SpinButton
            && n.numeric_value().is_some_and(|x| (x - v).abs() < within)
            && n.bounding_box().is_some_and(|b| (b.x0 < PANELS_LEFT) == by_the_box)
    })
}

/// The field of the numbers by the box that shows `v`, to within `within`.
fn box_field(v: f64, within: f64) -> egui_kittest::kittest::By<'static> {
    field_showing(v, within, true)
}

/// The widget with the keyboard focus.
fn focused() -> egui_kittest::kittest::By<'static> {
    egui_kittest::kittest::by().predicate(|n| n.is_focused())
}

/// Types `text` into `field`, in place of what it shows.
fn type_into(h: &mut Harness<'static, OpalApp>, field: egui_kittest::kittest::By<'static>, text: &str) {
    h.get(field).click();
    h.run();
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.get(focused()).type_text(text);
    h.run();
}

fn put_on_clipboard(h: &mut Harness<'static, OpalApp>, p: crate::clipboard::Pasted) {
    h.state_mut().state_mut().clipboard =
        Box::new(crate::clipboard::TestClipboard { image: Some(p), ..Default::default() });
}

fn image_of(h: &mut Harness<'static, OpalApp>, w: u32, ht: u32) {
    new_image(h, w, ht, opal_core::units::DEFAULT_PPI);
    h.run();
}

/// A new white image `w` × `ht` pixels at `ppi` (for the next run).
fn new_image(h: &mut Harness<'static, OpalApp>, w: u32, ht: u32, ppi: f64) {
    h.state_mut().state_mut().create_image(&crate::state::NewImage {
        width: w,
        height: ht,
        background: crate::state::Background::White,
        resolution: ppi,
    });
}

#[test]
fn ctrl_v_opens_the_box_and_enter_places_the_paste_as_one_state() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 400, 300);
    put_on_clipboard(&mut h, pasted(50, 40, [1.0, 0.0, 0.0, 1.0]));
    // Ctrl+V reaches the app as a paste event, not as a key.
    h.event(egui::Event::Paste(String::new()));
    h.run();
    let d = h.state().state().active().unwrap();
    assert!(d.transform.is_some(), "the box opened");
    assert_eq!(d.doc().layers.len(), 2, "on a new layer");
    let states = d.history.nodes().count();
    render(&mut h, "paste-box");
    h.key_press(egui::Key::Enter);
    h.run();
    let d = h.state().state().active().unwrap();
    assert!(d.transform.is_none() && d.pending.is_none());
    assert_eq!(d.history.nodes().count(), states + 1);
    assert_eq!(d.history.undo_label(), Some("Paste"));
    let top = &d.doc().layers[1];
    let opal_core::document::ObjectKind::Image(i) = &top.objects()[0].kind else { panic!("an image object") };
    // Centered on the canvas, on whole pixels.
    assert_eq!((i.transform.e, i.transform.f), (175.0, 130.0));
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    h.run();
    assert_eq!(h.state().state().active().unwrap().doc().layers.len(), 1, "undo takes the paste back");
}

#[test]
fn the_numbers_button_by_the_box_opens_its_fields_in_the_unit_chosen() {
    let _gpu = gpu_lock();
    let mut h = harness();
    pasting(&mut h);
    h.run();
    let angles = |h: &Harness<'static, OpalApp>| h.query_all_by_label("Angle").count();
    let before = angles(&h);
    h.get_by_label(&crate::icons::NUMBERS.to_string()).click();
    h.run();
    assert!(h.state().state().box_numbers, "the button did not open them");
    assert_eq!(angles(&h), before + 1, "the fields are not by the box");
    // In inches, at the image's resolution: the paste is 0.5 in wide.
    h.state_mut().state_mut().set_unit(opal_core::units::Unit::In);
    h.run();
    let shows = |h: &Harness<'static, OpalApp>, v: f64| {
        h.query_all(egui_kittest::kittest::by().predicate(move |n| n.numeric_value() == Some(v))).count()
    };
    // (By the box and in the tool settings, among other numbers that happen to be 0.5.)
    assert!(shows(&h, 0.5) >= 2, "the width is not shown in inches, by the box and in the panel");
    render(&mut h, "paste-numbers");
    // Fitted into a rectangle typed beside them, centered in it, in its own proportions.
    h.state_mut().state_mut().active_mut().unwrap().fit_rect = Some([0.0, 0.0, 100.0, 100.0]);
    h.run();
    // (By the box, which is the last: the tool settings show the same fields.)
    h.query_all_by_label("Fit inside").last().unwrap().click();
    h.run();
    let p = box_params(&h);
    assert_eq!((p.x, p.y, p.width, p.height), (0.0, 10.0, 100.0, 80.0));
    h.query_all_by_label("Fill").last().unwrap().click();
    h.run();
    let p = box_params(&h);
    assert_eq!((p.x, p.y, p.width, p.height), (-12.5, 0.0, 125.0, 100.0));
    // Its position given at its center: filling 100 × 100 from the canvas's corner, the
    // 125 × 100 box's top-left corner is 0.125 in left of it, and its center 0.5 in in.
    assert!(shows(&h, -0.125) >= 1, "the top-left corner is not shown");
    h.query_all_by_label("Center").last().unwrap().click();
    h.run();
    assert_eq!(h.state().state().prefs.choice("transform.reference"), "center");
    assert_eq!(shows(&h, -0.125), 0, "the top-left corner is still shown");
    assert!(shows(&h, 0.5) >= 2, "the center is not shown");
    // Counted from the canvas's bottom-right corner (4 × 3 in): the center is 3.5 in left of
    // it and 2.5 in above.
    h.query_all_by_label("From: Bottom right").last().unwrap().click();
    h.run();
    assert!(shows(&h, -3.5) >= 1 && shows(&h, -2.5) >= 1, "not counted from the canvas's bottom-right corner");
    // The crop box has them too.
    h.key_press(egui::Key::Enter);
    h.state_mut().state_mut().set_tool(crate::tools::ToolId::Crop);
    h.run();
    assert!(shows(&h, 4.0) >= 1, "the crop box's width is not shown");
    render(&mut h, "crop-numbers");
}

#[test]
fn escape_cancels_a_paste_and_restores_the_selection() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 200, 200);
    let bg = h.state().state().active().unwrap().active_layer().unwrap();
    put_on_clipboard(&mut h, pasted(10, 10, [0.0, 1.0, 0.0, 1.0]));
    h.state_mut().state_mut().queue.push(Cmd::Paste);
    h.run();
    h.key_press(egui::Key::Escape);
    h.run();
    let d = h.state().state().active().unwrap();
    assert!(d.transform.is_none() && d.pending.is_none());
    assert_eq!(d.doc().layers.len(), 1);
    assert_eq!(d.active_layer(), Some(bg));
    assert!(!d.is_dirty());
}

#[test]
fn dragging_inside_the_box_moves_the_paste() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 400, 300);
    put_on_clipboard(&mut h, pasted(100, 100, [0.0, 0.0, 1.0, 1.0]));
    h.state_mut().state_mut().queue.push(Cmd::Paste);
    h.run();
    let (v, start) = {
        let d = h.state().state().active().unwrap();
        (d.view, d.transform.as_ref().unwrap().current)
    };
    let ppp = h.ctx.pixels_per_point();
    let at = |x: f32, y: f32| v.screen_rect.min + v.to_screen(egui::vec2(x, y)) / ppp;
    // From the middle of the box, 30 canvas pixels right, with snapping off.
    h.state_mut().state_mut().prefs.set("transform.snapping", crate::settings::Value::Bool(false));
    h.hover_at(at(200.0, 150.0));
    h.run();
    h.drag_at(at(200.0, 150.0));
    h.run();
    for x in [205.0, 215.0, 230.0] {
        h.hover_at(at(x, 150.0));
        h.run();
    }
    h.drop_at(at(230.0, 150.0));
    h.run();
    let d = h.state().state().active().unwrap();
    let now = d.transform.as_ref().unwrap().current;
    assert!((now.e - start.e - 30.0).abs() <= 1.0, "moved {}", now.e - start.e);
    assert_eq!(now.f, start.f);
}

#[test]
fn copy_then_paste_in_place_puts_it_back_where_it_came_from() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 300, 200);
    h.state_mut().state_mut().clipboard = Box::new(crate::clipboard::TestClipboard::default());
    {
        let d = h.state_mut().state_mut().active_mut().unwrap();
        let mut img = opal_core::raster::RasterImage::new();
        for y in 20..30 {
            for x in 40..60 {
                img.set_pixel(x, y, opal_core::tile::pixel_from_f32([1.0, 0.0, 0.0, 1.0]));
            }
        }
        let _ = d.apply("Add", |doc, ids| {
            let l = opal_core::document::Layer::new_raster(ids.layer(), ids.object(), "dot", img);
            doc.insert(opal_core::document::Position { parent: None, index: 1 }, l)
        });
    }
    h.event(egui::Event::Copy);
    h.run();
    h.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::V);
    h.run();
    let d = h.state().state().active().unwrap();
    let s = d.transform.as_ref().expect("the box opened");
    assert_eq!((s.current.e, s.current.f), (40.0, 20.0), "in place");
    assert_eq!((s.frame.width(), s.frame.height()), (20.0, 10.0), "the copy was trimmed to the content");
}

#[test]
fn pasting_with_nothing_open_makes_a_new_image() {
    let _gpu = gpu_lock();
    let mut h = harness();
    h.run();
    put_on_clipboard(&mut h, pasted(33, 44, [0.5, 0.5, 0.5, 1.0]));
    h.event(egui::Event::Paste(String::new()));
    h.run();
    let d = h.state().state().active().expect("a new image");
    assert_eq!((d.doc().canvas.width, d.doc().canvas.height), (33, 44));
}

#[test]
fn transforming_a_pixel_layer_by_whole_pixels_leaves_it_pixels() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 100, 100);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::T);
    h.run();
    {
        let d = h.state_mut().state_mut().active_mut().unwrap();
        let m = d.transform.as_ref().expect("the box opened").current;
        d.set_box(m.then(opal_core::geom::Affine::translate(7.0, -3.0)), None);
    }
    h.key_press(egui::Key::Enter);
    h.run();
    let d = h.state().state().active().unwrap();
    let opal_core::document::ObjectKind::Pixels(p) = &d.doc().layers[0].objects()[0].kind else {
        panic!("a whole-pixel move keeps a pixel layer pixels")
    };
    assert_eq!(p.offset, [7, -3]);
    assert_eq!(d.history.undo_label(), Some("Transform"));
}

#[test]
fn merge_selected_combines_the_selected_layers() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 50, 50);
    h.state_mut().state_mut().queue.push(Cmd::NewLayer);
    h.run();
    let st = h.state_mut().state_mut();
    let d = st.active_mut().unwrap();
    let ids = d.doc().layer_ids();
    d.select_only(ids[0]);
    d.toggle_selected(ids[1]);
    st.queue.push(Cmd::MergeSelected);
    h.run();
    let d = h.state().state().active().unwrap();
    assert_eq!(d.doc().layers.len(), 1);
    assert_eq!(d.history.undo_label(), Some("Merge selected"));
}

#[test]
fn the_crop_tool_crops_to_its_box_on_enter() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 200, 100);
    h.key_press(egui::Key::C);
    h.run();
    {
        let d = h.state_mut().state_mut().active_mut().unwrap();
        assert!(d.crop.is_some(), "the box appears with the tool");
        d.crop = Some(crate::crop::CropBox { center: [50.0, 50.0], size: [60.0, 40.0], angle: 0.0 });
    }
    h.run();
    render(&mut h, "crop-box");
    h.key_press(egui::Key::Enter);
    h.run();
    let c = h.state().state().active().unwrap().doc().canvas;
    assert_eq!((c.x, c.y, c.width, c.height), (20, 30, 60, 40));
}

#[test]
fn space_keeps_the_crop_box_while_it_scrolls_the_view() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 200, 100);
    h.key_press(egui::Key::C);
    h.run();
    let b = crate::crop::CropBox { center: [60.0, 50.0], size: [50.0, 30.0], angle: 0.1 };
    h.state_mut().state_mut().active_mut().unwrap().crop = Some(b);
    h.event(egui::Event::Key {
        key: egui::Key::Space,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run();
    assert_eq!(h.state().state().active().unwrap().crop, Some(b), "the box was thrown away");
}

#[test]
fn opening_a_dialog_from_the_queue_asks_for_a_repaint() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 64, 64);
    let before = h.state().signature();
    h.state_mut().state_mut().run(Cmd::ResizeImage, &egui::Context::default());
    assert_ne!(h.state().signature(), before, "the dialog would wait for the pointer to move");
}

#[test]
fn a_slow_clipboard_is_read_off_the_interface_thread_and_pasted_when_ready() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 300, 200);
    let doc = h.state().state().active_doc;
    h.state_mut().state_mut().clipboard = Box::new(crate::clipboard::TestClipboard {
        image: Some(pasted(20, 10, [1.0; 4])),
        slow: true,
        ..Default::default()
    });
    h.state_mut().state_mut().queue.push(Cmd::Paste);
    h.run();
    // The worker thread delivers in its own time: wait for it, a frame at a time.
    for _ in 0..500 {
        if h.state().state().reading.is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.run();
    }
    let st = h.state().state();
    assert!(st.reading.is_none(), "the paste never arrived");
    assert_eq!(st.active_doc, doc);
    assert!(st.active().unwrap().transform.is_some(), "the box opened on the document pasted into");
}

#[test]
fn a_move_drag_released_while_space_is_held_ends() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 400, 300);
    h.key_press(egui::Key::V);
    h.run();
    let v = h.state().state().active().unwrap().view;
    let ppp = h.ctx.pixels_per_point();
    let at = |x: f32, y: f32| v.screen_rect.min + v.to_screen(egui::vec2(x, y)) / ppp;
    h.hover_at(at(200.0, 150.0));
    h.run();
    h.drag_at(at(200.0, 150.0));
    h.run();
    for x in [210.0, 230.0] {
        h.hover_at(at(x, 150.0));
        h.run();
    }
    assert!(h.state().state().active().unwrap().moving.is_some(), "the drag began");
    let space = |pressed| egui::Event::Key {
        key: egui::Key::Space,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    h.event(space(true));
    h.run();
    h.drop_at(at(230.0, 150.0));
    h.run();
    h.event(space(false));
    h.run();
    let d = h.state().state().active().unwrap();
    assert!(d.moving.is_none(), "the drag never ended, so the view stays a preview");
    assert!(d.pending.is_none());
    assert_eq!(d.history.undo_label(), Some("Move"));
}

/// Puts a clipboard on `h` that is read on a worker thread, holding a `w × h` image.
fn slow_clipboard(h: &mut Harness<'static, crate::app::OpalApp>, w: u32, ht: u32) {
    h.state_mut().state_mut().clipboard = Box::new(crate::clipboard::TestClipboard {
        image: Some(pasted(w, ht, [1.0; 4])),
        slow: true,
        ..Default::default()
    });
}

/// Runs frames until the paste being read has come back (or been placed).
fn until_read(h: &mut Harness<'static, crate::app::OpalApp>) {
    for _ in 0..500 {
        if h.state().state().reading.as_ref().is_none_or(|r| r.result.is_some()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.run();
    }
    panic!("the clipboard was never read");
}

#[test]
fn a_late_paste_waits_for_a_dialog_opened_meanwhile() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 300, 200);
    slow_clipboard(&mut h, 20, 10);
    let st = h.state_mut().state_mut();
    st.queue.push(Cmd::Paste);
    st.queue.push(Cmd::ResizeImage);
    h.run();
    until_read(&mut h);
    h.run();
    let st = h.state().state();
    assert!(st.resize_image.is_some());
    assert!(st.active().unwrap().transform.is_none(), "the paste landed behind the dialog");
    h.state_mut().state_mut().resize_image = None;
    h.run();
    let st = h.state().state();
    assert!(st.reading.is_none());
    assert_eq!(st.active().unwrap().pending_label(), Some("Paste"), "placed once the dialog closed");
}

#[test]
fn a_late_paste_waits_for_a_box_opened_meanwhile() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 300, 200);
    slow_clipboard(&mut h, 20, 10);
    let st = h.state_mut().state_mut();
    st.queue.push(Cmd::Paste);
    st.queue.push(Cmd::Transform);
    h.run();
    until_read(&mut h);
    h.run();
    let st = h.state().state();
    assert_eq!(st.active().unwrap().pending_label(), Some("Transform"), "the box opened meanwhile was ended");
    assert!(st.notice.as_ref().is_some_and(|n| n.0.contains("transform box closes")), "{:?}", st.notice);
    h.key_press(egui::Key::Escape);
    h.run();
    let st = h.state().state();
    assert!(st.reading.is_none());
    assert_eq!(st.active().unwrap().pending_label(), Some("Paste"));
    assert!(st.notice.is_none(), "the waiting notice stayed up");
}

#[test]
fn a_late_paste_for_a_closed_image_is_dropped() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 300, 200);
    slow_clipboard(&mut h, 20, 10);
    let st = h.state_mut().state_mut();
    st.queue.push(Cmd::Paste);
    st.queue.push(Cmd::Close);
    h.run();
    until_read(&mut h);
    h.run();
    let st = h.state().state();
    assert!(st.reading.is_none());
    assert!(st.docs.is_empty(), "a new image was opened");
    assert!(st.notice.as_ref().is_some_and(|n| n.0.contains("closed")), "{:?}", st.notice);
}

#[test]
fn a_paste_read_at_once_replaces_one_still_being_read() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 300, 200);
    slow_clipboard(&mut h, 20, 10);
    let ctx = h.ctx.clone();
    let st = h.state_mut().state_mut();
    st.run(Cmd::Paste, &ctx);
    st.clipboard =
        Box::new(crate::clipboard::TestClipboard { image: Some(pasted(7, 5, [1.0; 4])), ..Default::default() });
    st.run(Cmd::Paste, &ctx);
    assert!(st.reading.is_none(), "the earlier paste would land later");
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.run();
    }
    let s = h.state().state().active().unwrap().transform.as_ref().unwrap();
    assert_eq!((s.frame.width(), s.frame.height()), (7.0, 5.0));
}

#[test]
fn a_late_paste_leaves_other_notices_up() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 300, 200);
    slow_clipboard(&mut h, 20, 10);
    h.state_mut().state_mut().queue.push(Cmd::Paste);
    h.run();
    h.state_mut().state_mut().notify("Copied 300 × 200 pixels.");
    until_read(&mut h);
    h.run();
    let st = h.state().state();
    assert!(st.reading.is_none());
    assert_eq!(st.notice.as_ref().map(|n| n.0.as_str()), Some("Copied 300 × 200 pixels."));
}

#[test]
fn a_late_paste_waits_for_a_crop_box_changed_meanwhile() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 300, 200);
    h.key_press(egui::Key::C);
    h.run();
    // A paste asked for with the crop box showing, which has come back.
    let read = |st: &crate::state::AppState| crate::state::Reading {
        rx: std::sync::mpsc::channel().1,
        result: Some(Ok(Some(pasted(20, 10, [1.0; 4])))),
        in_place: false,
        target: st.active_doc,
        box_then: None,
        crop_then: st.active().unwrap().crop,
        notice: String::new(),
        answer: None,
    };
    let st = h.state_mut().state_mut();
    st.reading = Some(read(st));
    // Left as it was, the crop box goes, as it would for a paste read at once.
    h.run();
    assert_eq!(h.state().state().active().unwrap().pending_label(), Some("Paste"));
    h.key_press(egui::Key::Escape);
    h.run();
    // Changed meanwhile, it is the user's work: the paste waits until they are done.
    let st = h.state_mut().state_mut();
    assert!(st.active().unwrap().crop.is_some(), "the crop tool is still chosen");
    st.reading = Some(read(st));
    st.active_mut().unwrap().crop = Some(crate::crop::CropBox { center: [50.0, 50.0], size: [60.0, 40.0], angle: 0.0 });
    h.run();
    assert!(h.state().state().active().unwrap().transform.is_none(), "the crop box was thrown away");
    h.key_press(egui::Key::V);
    h.run();
    assert_eq!(h.state().state().active().unwrap().pending_label(), Some("Paste"), "placed once the tool changed");
}

#[test]
fn the_history_panel_says_what_history_holds() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 600, 400);
    h.state_mut().state_mut().queue.push(Cmd::FlipHorizontal);
    h.run();
    h.get_by_label(&format!("{}  History", crate::icons::HISTORY)).click();
    h.run();
    // The flip made a copy of every tile: the state before holds them alone.
    let expected = opal_io::bytes(h.state().state().active().unwrap().history.memory());
    assert_ne!(expected, "0 KB");
    assert!(h.query_by_label(&expected).is_some(), "the panel does not show {expected}");
    render(&mut h, "history-panel");
}

#[test]
fn letting_go_of_the_box_draws_exactly_at_once() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 800, 600);
    put_on_clipboard(&mut h, pasted(200, 200, [0.0, 0.0, 1.0, 1.0]));
    // Zoomed out, so drags draw previews; with the chosen method while dragging.
    let st = h.state_mut().state_mut();
    st.prefs.set("transform.draft", crate::settings::Value::Bool(false));
    st.prefs.set("transform.snapping", crate::settings::Value::Bool(false));
    st.active_mut().unwrap().view.zoom = 0.5;
    st.queue.push(Cmd::Paste);
    h.run();
    let v = h.state().state().active().unwrap().view;
    let ppp = h.ctx.pixels_per_point();
    let at = |x: f32, y: f32| v.screen_rect.min + v.to_screen(egui::vec2(x, y)) / ppp;
    h.hover_at(at(400.0, 300.0));
    h.run();
    h.drag_at(at(400.0, 300.0));
    h.run();
    for x in [420.0, 460.0] {
        h.hover_at(at(x, 300.0));
        h.run();
    }
    assert!(h.state().state().active().unwrap().projector.shows_previews(), "the drag draws previews");
    // The frame the button is let go, alone: nothing promises another.
    h.event(egui::Event::PointerButton {
        pos: at(460.0, 300.0),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
    assert!(!h.state().state().active().unwrap().projector.shows_previews(), "the previews stay after the drag");
}

/// A paste for document `target` whose reading has come back with `result`, asked for
/// when `target`'s box (if any) and crop box were as they are now.
fn read_back(st: &crate::state::AppState, result: crate::clipboard::ReadResult) -> crate::state::Reading {
    let d = st.active();
    crate::state::Reading {
        rx: std::sync::mpsc::channel().1,
        result: Some(result),
        in_place: false,
        target: st.active_doc,
        box_then: d.and_then(|d| d.transform.as_ref()).map(|s| s.serial),
        crop_then: d.and_then(|d| d.crop),
        notice: String::new(),
        answer: None,
    }
}

#[test]
fn a_new_paste_takes_down_what_the_one_it_replaces_said() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 300, 200);
    let ctx = h.ctx.clone();
    let st = h.state_mut().state_mut();
    let mut waiting = read_back(st, Ok(Some(pasted(20, 10, [1.0; 4]))));
    waiting.notice = "The paste will be placed when the transform box closes.".into();
    st.notify(waiting.notice.clone());
    st.reading = Some(waiting);
    st.clipboard =
        Box::new(crate::clipboard::TestClipboard { image: Some(pasted(7, 5, [1.0; 4])), ..Default::default() });
    st.run(Cmd::Paste, &ctx);
    assert!(st.notice.is_none(), "{:?}", st.notice);
}

#[test]
fn a_box_opened_after_the_one_open_when_pasting_is_waited_for() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 300, 200);
    h.state_mut().state_mut().queue.push(Cmd::Transform);
    h.run();
    let ctx = h.ctx.clone();
    let st = h.state_mut().state_mut();
    st.reading = Some(read_back(st, Ok(Some(pasted(20, 10, [1.0; 4])))));
    // That box is applied, and another opened, before the paste is placed.
    st.active_mut().unwrap().commit_pending();
    st.run(Cmd::Transform, &ctx);
    h.run();
    let st = h.state().state();
    assert!(st.reading.is_some(), "the paste ended the newer box");
    assert_eq!(st.active().unwrap().pending_label(), Some("Transform"));
}

#[test]
fn a_paste_for_a_closed_image_says_nothing_about_the_clipboard() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 300, 200);
    let st = h.state_mut().state_mut();
    st.reading = Some(read_back(st, Ok(None)));
    st.queue.push(Cmd::Close);
    h.run();
    let st = h.state().state();
    assert!(st.messages.is_empty(), "a message about a clipboard nobody is pasting from");
    assert!(st.notice.as_ref().is_some_and(|n| n.0.contains("closed")), "{:?}", st.notice);
}

/// Runs frames until the writes asked for are done.
fn until_written(h: &mut Harness<'static, crate::app::OpalApp>) {
    for _ in 0..500 {
        if !h.state().state().writer.busy() {
            h.run();
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.run();
    }
    panic!("the writes never finished");
}

#[test]
fn a_save_is_written_off_the_interface_thread_and_marks_the_image_clean() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 300, 200);
    let path = std::env::temp_dir().join(format!("opal-save-test-{}.opal", std::process::id()));
    let ctx = h.ctx.clone();
    let st = h.state_mut().state_mut();
    let id = st.active_doc.unwrap();
    st.active_mut().unwrap().path = Some(path.clone());
    st.active_mut().unwrap().saved_at = None;
    assert!(st.save(id, false, crate::writer::Then::Nothing, &ctx));
    until_written(&mut h);
    let st = h.state().state();
    assert!(!st.active().unwrap().is_dirty(), "the image is not marked saved");
    assert!(st.notice.as_ref().is_some_and(|n| n.0.starts_with("Saved")), "{:?}", st.notice);
    let loaded = opal_io::native::load(&path, None).expect("the file reads back");
    assert_eq!(loaded.doc.canvas.width, 300);
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn quitting_waits_for_the_files_being_written() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 64, 64);
    let ctx = h.ctx.clone();
    let (job, release) = held_result(false);
    let st = h.state_mut().state_mut();
    let path = std::env::temp_dir().join("never-written.png");
    st.writer.write(crate::writer::Written::Export { path }, crate::writer::Then::Nothing, job, &ctx);
    st.queue.push(Cmd::Exit);
    h.run();
    let st = h.state().state();
    assert!(!st.exit_confirmed, "quit while a file was being written");
    assert!(st.exit_after_writes);
    drop(release);
    until_written(&mut h);
    h.run();
    assert!(h.state().state().exit_confirmed, "did not quit once the file was written");
}

#[test]
fn saving_then_closing_closes_once_written_and_asks_again_if_edited_meanwhile() {
    let _gpu = gpu_lock();
    for edit in [false, true] {
        let mut h = harness();
        image_of(&mut h, 64, 64);
        let ctx = h.ctx.clone();
        let (job, release) = held_result(false);
        let st = h.state_mut().state_mut();
        let id = st.active_doc.unwrap();
        let at = st.active().unwrap().history.current();
        let path = std::env::temp_dir().join("never-saved.opal");
        st.active_mut().unwrap().path = Some(path.clone());
        let written = crate::writer::Written::Save { doc: id, at, path, before: (None, "Untitled".into()) };
        st.writer.write(written, crate::writer::Then::Close, job, &ctx);
        if edit {
            st.queue.push(Cmd::NewLayer);
        }
        h.run();
        assert!(h.state().state().doc(id).is_some(), "closed before it was written");
        drop(release);
        until_written(&mut h);
        let st = h.state().state();
        if edit {
            assert!(st.doc(id).is_some(), "the edit made meanwhile was thrown away");
            assert!(matches!(st.confirm, Some(crate::state::Confirm::Close(c)) if c == id), "not asked again");
        } else {
            assert!(st.doc(id).is_none(), "not closed once written");
        }
    }
}

#[test]
fn saving_all_then_quitting_quits_once_every_save_is_written() {
    let _gpu = gpu_lock();
    for fails in [false, true] {
        saving_all_then_quitting(fails);
    }
}

/// Two saves to quit after; with `fails`, the second fails.
fn saving_all_then_quitting(fails: bool) {
    let mut h = harness();
    image_of(&mut h, 64, 64);
    let ctx = h.ctx.clone();
    let (first, release_first) = held_result(false);
    let (second, release_second) = held_result(fails);
    let st = h.state_mut().state_mut();
    let id = st.active_doc.unwrap();
    let at = st.active().unwrap().history.current();
    for job in [first, second] {
        let path = std::env::temp_dir().join("never-saved.opal");
        let written = crate::writer::Written::Save { doc: id, at, path, before: (None, "Untitled".into()) };
        st.writer.write(written, crate::writer::Then::Nothing, job, &ctx);
    }
    // As "Save all" in the quit question leaves it.
    st.exit_after_writes = true;
    drop(release_first);
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.run();
    }
    assert!(!h.state().state().exit_confirmed, "quit before the last save was written");
    drop(release_second);
    until_written(&mut h);
    h.run();
    let st = h.state().state();
    if fails {
        assert!(!st.exit_confirmed, "quit although a save failed");
        assert!(st.messages.iter().any(|m| m.title == "Could not save"), "the failure was not said");
    } else {
        assert!(st.exit_confirmed, "did not quit once every save was written");
    }
}

#[test]
fn the_first_drop_of_history_is_said_once_the_status_bar_is_free() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 64, 64);
    {
        let d = h.state_mut().state_mut().active_mut().unwrap();
        // Painting the background: each state's tile is its own.
        let bg = d.doc().layers[0].id;
        for x in 0..3 {
            let _ = d.apply::<()>("Paint", |doc, _| {
                let opal_core::document::LayerContent::Objects(objects) = &mut doc.layer_mut(bg).unwrap().content
                else {
                    panic!()
                };
                let opal_core::document::ObjectKind::Pixels(p) = &mut objects[0].kind else { panic!() };
                std::sync::Arc::make_mut(&mut p.image).set_pixel(x, 0, opal_core::tile::pixel_from_f32([1.0; 4]));
                Ok(())
            });
        }
        // An operation's own notice, given in the frame the states are dropped.
        d.notice = Some("Some of it was clipped.".into());
        d.set_history_budget(0);
        assert!(d.drop_note_due());
    }
    h.run();
    let st = h.state().state();
    assert_eq!(st.notice.as_ref().map(|n| n.0.as_str()), Some("Some of it was clipped."), "the notice was hidden");
    assert!(st.active().unwrap().drop_note_due());
    // Once that notice has had its time, the drop is said.
    let st = h.state_mut().state_mut();
    let long_ago = std::time::Instant::now() - crate::state::NOTICE_SHOWN - std::time::Duration::from_secs(1);
    st.notice.as_mut().unwrap().1 = long_ago;
    h.run();
    let st = h.state().state();
    assert_eq!(st.notice.as_ref().map(|n| n.0.as_str()), Some(crate::doc::DROP_NOTE));
    assert!(!st.active().unwrap().drop_note_due());
}

/// A write that waits until the sender returned is dropped, then fails or not.
fn held_result(fails: bool) -> (crate::writer::Job, std::sync::mpsc::Sender<()>) {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    (
        Box::new(move |_: &opal_io::Progress| {
            let _ = rx.recv();
            if fails { Err("the disk is full".into()) } else { Ok(()) }
        }),
        tx,
    )
}

/// A write a quarter done that waits until the sender returned is dropped, then fails if it
/// was cancelled meanwhile (as a real write stops at its next step), else succeeds.
fn cancellable_write() -> (crate::writer::Job, std::sync::mpsc::Sender<()>) {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    (
        Box::new(move |p: &opal_io::Progress| {
            p.begin(4);
            p.advance(1);
            let _ = rx.recv();
            if p.is_cancelled() { Err("cancelled".into()) } else { Ok(()) }
        }),
        tx,
    )
}

#[test]
fn a_write_under_way_shows_how_far_it_has_got_and_can_be_cancelled() {
    let _gpu = gpu_lock();
    for save in [true, false] {
        let mut h = harness();
        image_of(&mut h, 64, 64);
        // An edit to save, which a cancelled save leaves unsaved.
        h.state_mut().state_mut().queue.push(Cmd::NewLayer);
        h.run();
        let ctx = h.ctx.clone();
        let (job, release) = cancellable_write();
        let st = h.state_mut().state_mut();
        let id = st.active_doc.unwrap();
        assert!(st.active().unwrap().is_dirty());
        let (before, title) = (std::env::temp_dir().join("before.opal"), "before".to_string());
        let dir = std::env::temp_dir();
        if save {
            // As Save As to another file, to be closed once written, and then quit.
            let d = st.active_mut().unwrap();
            let (at, path) = (d.history.current(), dir.join("never-saved.opal"));
            d.path = Some(path.clone());
            d.title = "never-saved".into();
            let written =
                crate::writer::Written::Save { doc: id, at, path, before: (Some(before.clone()), title.clone()) };
            st.writer.write(written, crate::writer::Then::Close, job, &ctx);
            st.queue.push(Cmd::Exit);
        } else {
            let written = crate::writer::Written::Export { path: dir.join("never-exported.png") };
            st.writer.write(written, crate::writer::Then::Nothing, job, &ctx);
        }
        let mut shown = false;
        for _ in 0..500 {
            h.run();
            if h.query_by_label("25%").is_some() {
                shown = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(shown, "the status bar does not say how far the write has got");
        if save {
            render(&mut h, "saving");
        }
        h.get_by_label("Cancel").click();
        h.run();
        assert!(h.query_by_label("Cancelling…").is_some(), "not said to be cancelling");
        drop(release);
        until_written(&mut h);
        let st = h.state().state();
        assert!(st.messages.is_empty(), "a cancel reported as a failure: {:?}", st.messages.front().map(|m| &m.title));
        let notice = st.notice.as_ref().map(|n| n.0.as_str()).unwrap_or_default();
        assert!(notice.contains("was cancelled"), "{notice:?}");
        assert!(h.query_by_label("Cancel").is_none(), "still offered once done");
        if save {
            let d = st.doc(id).expect("closed though its save was cancelled");
            assert_eq!((d.path.as_ref(), &d.title), (Some(&before), &title), "the file it had was not given back");
            assert!(d.is_dirty());
            assert!(!st.exit_confirmed && !st.exit_after_writes && st.confirm.is_none(), "went on quitting");
        }
    }
}

#[test]
fn a_failed_write_stops_a_quit_waiting_for_it() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 64, 64);
    let ctx = h.ctx.clone();
    let (job, release) = held_result(true);
    let st = h.state_mut().state_mut();
    let path = std::env::temp_dir().join("never-written.png");
    st.writer.write(crate::writer::Written::Export { path }, crate::writer::Then::Nothing, job, &ctx);
    st.queue.push(Cmd::Exit);
    h.run();
    drop(release);
    until_written(&mut h);
    h.run();
    let st = h.state().state();
    assert!(!st.exit_confirmed, "quit, hiding the failure");
    assert!(st.messages.iter().any(|m| m.title == "Could not export"));
}

#[test]
fn a_failure_reported_with_the_last_save_stops_saving_all_then_quitting() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 64, 64);
    let ctx = h.ctx.clone();
    let st = h.state_mut().state_mut();
    let id = st.active_doc.unwrap();
    let at = st.active().unwrap().history.current();
    let mut releases = Vec::new();
    for fails in [true, false] {
        let (job, release) = held_result(fails);
        let path = std::env::temp_dir().join("never-saved.opal");
        let written = crate::writer::Written::Save { doc: id, at, path, before: (None, "Untitled".into()) };
        st.writer.write(written, crate::writer::Then::Nothing, job, &ctx);
        releases.push(release);
    }
    st.exit_after_writes = true;
    // Both done before the next frame looks.
    drop(releases);
    std::thread::sleep(std::time::Duration::from_millis(200));
    until_written(&mut h);
    h.run();
    let st = h.state().state();
    assert!(!st.exit_confirmed && st.confirm.is_none(), "went on quitting past a failed save");
    assert!(st.messages.iter().any(|m| m.title == "Could not save"));
}

#[test]
fn a_save_takes_its_file_at_once_and_gives_it_back_if_it_fails() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 64, 64);
    let ctx = h.ctx.clone();
    let dir = std::env::temp_dir();
    let (a, b) = (
        dir.join(format!("opal-a-{}.opal", std::process::id())),
        dir.join(format!("opal-b-{}.opal", std::process::id())),
    );
    let st = h.state_mut().state_mut();
    let id = st.active_doc.unwrap();
    st.active_mut().unwrap().path = Some(a.clone());
    st.save_to(id, b.clone(), crate::writer::Then::Nothing, &ctx);
    assert_eq!(st.active().unwrap().path.as_ref(), Some(&b), "the file chosen is not the image's yet");
    // Saving again before that is written goes to the new file.
    assert!(st.save(id, false, crate::writer::Then::Nothing, &ctx));
    let to: Vec<std::path::PathBuf> = st
        .writer
        .pending()
        .filter_map(|(w, _)| match w {
            crate::writer::Written::Save { path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect();
    assert!(to.iter().all(|p| *p == b), "{to:?}");
    until_written(&mut h);
    std::fs::remove_file(&b).unwrap();
    // A save that fails gives the image its file back.
    let nowhere = dir.join("no such folder").join("x.opal");
    let st = h.state_mut().state_mut();
    st.save_to(id, nowhere, crate::writer::Then::Nothing, &ctx);
    until_written(&mut h);
    let st = h.state().state();
    assert_eq!(st.active().unwrap().path.as_ref(), Some(&b), "the failed file was kept");
    assert!(st.messages.iter().any(|m| m.title == "Could not save"));
}

#[test]
fn closing_waits_its_turn_and_takes_its_question_with_it() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 64, 64);
    image_of(&mut h, 32, 32);
    let ctx = h.ctx.clone();
    let st = h.state_mut().state_mut();
    let (a, b) = (st.docs[0].id, st.docs[1].id);
    // `a` to close once saved: asking to close it again only says so.
    let (job, release) = held_result(false);
    let at = st.doc(a).unwrap().history.current();
    let path = std::env::temp_dir().join("never-saved.opal");
    st.doc_mut(a).unwrap().path = Some(path.clone());
    let written = crate::writer::Written::Save { doc: a, at, path, before: (None, "a".into()) };
    st.writer.write(written, crate::writer::Then::Close, job, &ctx);
    st.request_close(a);
    assert!(st.confirm.is_none(), "asked again about an image closing once saved");
    // A question about `b` on screen, then `a` changed while it was written: its question
    // waits for `b`'s to be answered rather than replacing it.
    st.doc_mut(b).unwrap().saved_at = None;
    st.request_close(b);
    let _ = st.doc_mut(a).unwrap().apply::<()>("Hide", |d, _| {
        let l = d.layers[0].id;
        d.layer_mut(l).unwrap().visible = false;
        Ok(())
    });
    drop(release);
    until_written(&mut h);
    let st = h.state_mut().state_mut();
    assert_eq!(st.confirm, Some(crate::state::Confirm::Close(b)), "the question on screen was replaced");
    // Closing `b` takes its question with it, and `a`'s comes.
    st.close_now(b);
    assert!(st.confirm.is_none());
    h.run();
    assert_eq!(h.state().state().confirm, Some(crate::state::Confirm::Close(a)));
}

#[test]
fn a_close_after_saving_waits_for_the_button_to_be_let_go() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 64, 64);
    let ctx = h.ctx.clone();
    let (job, release) = held_result(false);
    let st = h.state_mut().state_mut();
    let id = st.active_doc.unwrap();
    let at = st.active().unwrap().history.current();
    let path = std::env::temp_dir().join("never-saved.opal");
    st.active_mut().unwrap().path = Some(path.clone());
    let written = crate::writer::Written::Save { doc: id, at, path, before: (None, "Untitled".into()) };
    st.writer.write(written, crate::writer::Then::Close, job, &ctx);
    // Changed while it is written, so closing asks again; meanwhile a drag goes on.
    st.queue.push(Cmd::NewLayer);
    h.run();
    let at = h.state().state().active().unwrap().view.screen_rect.center();
    h.drag_at(at);
    h.run();
    drop(release);
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.run();
    }
    assert!(h.state().state().confirm.is_none(), "asked in the middle of a drag");
    h.drop_at(at);
    h.run();
    assert_eq!(h.state().state().confirm, Some(crate::state::Confirm::Close(id)));
}

#[test]
fn quitting_waits_for_a_close_question_on_screen() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 64, 64);
    image_of(&mut h, 32, 32);
    let st = h.state_mut().state_mut();
    let (a, b) = (st.docs[0].id, st.docs[1].id);
    for id in [a, b] {
        st.doc_mut(id).unwrap().saved_at = None;
    }
    st.confirm = Some(crate::state::Confirm::Close(a));
    st.exit_requested = true;
    h.run();
    assert_eq!(h.state().state().confirm, Some(crate::state::Confirm::Close(a)), "the close question was replaced");
    // Answered, the quit asks about what is left.
    h.state_mut().state_mut().close_now(a);
    h.run();
    assert_eq!(h.state().state().confirm, Some(crate::state::Confirm::Exit(vec![b])));
}

#[test]
fn two_saves_as_that_fail_give_back_the_file_the_image_had() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 64, 64);
    let ctx = h.ctx.clone();
    let dir = std::env::temp_dir();
    let a = dir.join("a.opal");
    let nowhere = dir.join("no such folder");
    let st = h.state_mut().state_mut();
    let id = st.active_doc.unwrap();
    st.active_mut().unwrap().path = Some(a.clone());
    // Both asked for before either is reported.
    st.save_to(id, nowhere.join("b.opal"), crate::writer::Then::Nothing, &ctx);
    st.save_to(id, nowhere.join("c.opal"), crate::writer::Then::Nothing, &ctx);
    until_written(&mut h);
    let st = h.state().state();
    assert_eq!(st.active().unwrap().path.as_ref(), Some(&a), "left with a file that was never written");
    assert_eq!(st.messages.iter().filter(|m| m.title == "Could not save").count(), 2);
}

#[test]
fn a_save_as_failing_while_a_later_one_is_written_passes_its_file_on() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 64, 64);
    let ctx = h.ctx.clone();
    let dir = std::env::temp_dir();
    let (a, b, c) = (dir.join("a.opal"), dir.join("no such folder").join("b.opal"), dir.join("c.opal"));
    let st = h.state_mut().state_mut();
    let id = st.active_doc.unwrap();
    st.active_mut().unwrap().path = Some(a.clone());
    st.save_to(id, b.clone(), crate::writer::Then::Nothing, &ctx);
    // Saved as c before b is reported, the save to c held until b's failure is.
    let (job, release) = held_result(true);
    let at = st.active().unwrap().history.current();
    st.active_mut().unwrap().path = Some(c.clone());
    let written = crate::writer::Written::Save { doc: id, at, path: c, before: (Some(b), "b".into()) };
    st.writer.write(written, crate::writer::Then::Nothing, job, &ctx);
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.run();
    }
    assert_eq!(h.state().state().messages.len(), 1, "b's failure not reported first");
    drop(release);
    until_written(&mut h);
    assert_eq!(h.state().state().active().unwrap().path.as_ref(), Some(&a));
}

#[test]
fn escape_cancels_a_paste_being_read_unless_a_box_takes_it() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 64, 64);
    // A reading still under way (its sender kept, so it never ends).
    let (tx, rx) = std::sync::mpsc::channel();
    let reading = |st: &crate::state::AppState, rx| crate::state::Reading {
        rx,
        result: None,
        in_place: false,
        target: st.active_doc,
        box_then: None,
        crop_then: None,
        notice: String::new(),
        answer: None,
    };
    let st = h.state_mut().state_mut();
    st.reading = Some(reading(st, rx));
    let notice = |h: &Harness<'static, OpalApp>| h.state().state().notice.as_ref().map(|n| n.0.clone());
    let says_esc = "Reading the clipboard… (Esc cancels the paste)";
    h.run();
    assert_eq!(notice(&h).as_deref(), Some(says_esc));
    // With a box open, Esc is the box's, and the status bar does not say otherwise.
    h.state_mut().state_mut().queue.push(Cmd::Transform);
    h.run();
    assert_eq!(notice(&h).as_deref(), Some("Reading the clipboard…"));
    h.key_press(egui::Key::Escape);
    h.run();
    let st = h.state().state();
    assert!(st.active().unwrap().transform.is_none(), "the box stayed");
    assert!(st.reading.is_some(), "the paste went with the box");
    assert_eq!(notice(&h).as_deref(), Some(says_esc));
    // Then Esc cancels the paste, which never lands.
    h.key_press(egui::Key::Escape);
    h.run();
    let st = h.state().state();
    assert!(st.reading.is_none(), "Esc did not cancel the paste");
    assert_eq!(st.notice.as_ref().map(|n| n.0.as_str()), Some("Paste cancelled."));
    let _ = tx.send(Ok(Some(pasted(10, 10, [1.0; 4]))));
    h.run();
    assert!(h.state().state().active().unwrap().transform.is_none(), "the cancelled paste landed");
}

#[test]
fn a_view_taking_a_while_says_so() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 256, 256);
    h.state_mut().state_mut().active_mut().unwrap().projector.wait = std::time::Duration::ZERO;
    // The background threads held, so the next view cannot be done.
    let held = crate::background::testing::hold(Some(crate::background::pool()));
    h.state_mut().state_mut().queue.push(Cmd::FlipHorizontal);
    h.run();
    assert!(h.query_by_label("Drawing…").is_none(), "said at once");
    std::thread::sleep(std::time::Duration::from_millis(200));
    // The spinner keeps repainting: frames one at a time.
    h.step();
    h.step();
    assert!(h.query_by_label("Drawing…").is_some(), "the view taking a while did not say so");
    render(&mut h, "drawing");
    // The sign takes no presses: a drag begun on it pans the canvas.
    h.state_mut().state_mut().set_tool(crate::tools::ToolId::Hand);
    let before = h.state().state().active().unwrap().view.offset;
    let on_sign = h.get_by_label("Drawing…").rect().center();
    let to = on_sign + egui::vec2(-60.0, 40.0);
    h.hover_at(on_sign);
    h.step();
    h.drag_at(on_sign);
    h.step();
    h.hover_at(to);
    h.step();
    h.event(egui::Event::PointerButton {
        pos: to,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
    assert_ne!(
        h.state().state().active().unwrap().view.offset,
        before,
        "a drag begun on the sign did not reach the canvas"
    );
    drop(held);
    for _ in 0..200 {
        if h.state().state().active().unwrap().projector.behind_for().is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.run();
    assert!(h.query_by_label("Drawing…").is_none(), "still said once the view was done");
}

#[test]
fn the_export_dialog_says_when_a_jpeg_would_be_too_large() {
    use egui_kittest::kittest::NodeT;
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 70_000, 100);
    let st = h.state_mut().state_mut();
    st.last_export = crate::state::ExportKind::Jpeg;
    st.queue.push(Cmd::Export);
    h.run();
    let said = "A JPEG image can be at most 65535 pixels wide and tall: choose PNG, or a smaller size.";
    assert!(h.query_by_label(said).is_some(), "not said");
    assert!(h.get_by_label("Export…").accesskit_node().is_disabled(), "offered all the same");
    // As PNG, it can be.
    h.get_by_label("PNG").click();
    h.run();
    assert!(h.query_by_label(said).is_none());
    assert!(!h.get_by_label("Export…").accesskit_node().is_disabled());
}

#[test]
fn the_largest_canvas_fits_the_window() {
    let mut v = crate::doc::View::default();
    let side = crate::dialogs::MAX_SIDE as f32;
    let viewport = egui::vec2(600.0, 400.0);
    v.fit(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(side, side)), viewport, 1.0);
    assert!(side * v.zoom <= viewport.y, "zoom {} leaves it {} pixels tall", v.zoom, side * v.zoom);
}

#[test]
fn the_console_offers_the_scripts_run_lately() {
    use egui_kittest::kittest::NodeT;
    let _gpu = gpu_lock();
    let mut h = harness();
    // One there (any file will do: none is run), and one gone.
    let here = std::env::current_exe().unwrap();
    let gone = here.with_file_name("no-such-script.py");
    let st = h.state_mut().state_mut();
    st.console.recent = vec![here.clone(), gone.clone()];
    st.scripts_file = Some(std::env::temp_dir().join("opal-ui-recent.json"));
    st.show_console = true;
    h.run();
    // Whether each is there is looked at off the interface thread: once it has been, it shows.
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while h.state().state().console.there(&gone) && std::time::Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.run();
    let name = here.file_name().unwrap().to_string_lossy().into_owned();
    let again = format!("{}  Run {name} again", crate::icons::PLAY);
    assert!(!h.get_by_label(&again).accesskit_node().is_disabled(), "the last one is not offered again");
    h.get_by_label(&format!("{}  Recent", crate::icons::HISTORY)).click();
    h.run();
    assert!(!h.get_by_label(&name).accesskit_node().is_disabled());
    assert!(h.get_by_label("no-such-script.py").accesskit_node().is_disabled(), "one gone is offered");
    h.get_by_label("Forget them").click();
    h.run();
    assert!(h.state().state().console.recent.is_empty());
}

// ---- Selections ------------------------------------------------------------------------

/// Makes the rectangle from `(x0, y0)` to `(x1, y1)` what is selected in the active image.
fn select_rect(h: &mut Harness<'static, OpalApp>, [x0, y0, x1, y1]: [f64; 4]) {
    use opal_core::mask::{Combine, Shape, ShapeKind};
    use opal_core::selection::{Selection, Step};
    let shape = Shape {
        kind: ShapeKind::Rectangle,
        center: [(x0 + x1) / 2.0, (y0 + y1) / 2.0],
        size: [x1 - x0, y1 - y0],
        angle: 0.0,
        smooth: true,
    };
    let d = h.state_mut().state_mut().active_mut().expect("an image is open");
    d.set_pixel_selection(Some(Selection::new(Step::Shape { shape, combine: Combine::Replace })), "Select");
    h.run();
}

/// Runs `cmd`, as its button would.
fn command(h: &mut Harness<'static, OpalApp>, cmd: Cmd) {
    h.state_mut().state_mut().queue.push(cmd);
    h.run();
}

/// How much of the active image is selected, if anything is.
fn selected(h: &Harness<'static, OpalApp>) -> Option<opal_core::mask::Extent> {
    h.state().state().active().unwrap().pixel_selection().map(|s| s.mask().extent())
}

#[test]
fn select_all_deselect_and_invert_change_what_is_selected_as_history_states() {
    use opal_core::geom::IRect;
    use opal_core::mask::Extent;
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 300, 200);
    // By its shortcut: the canvas, outlined round its edge.
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run();
    assert_eq!(selected(&h), Some(Extent::Within(IRect::new(0, 0, 300, 200))));
    let outline = h.state_mut().state_mut().active_mut().unwrap().selection_outline().unwrap();
    let mut corners = outline[0].clone();
    corners.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(outline.len(), 1);
    assert_eq!(corners, [[0.0, 0.0], [0.0, 200.0], [300.0, 0.0], [300.0, 200.0]]);
    command(&mut h, Cmd::InvertSelection);
    assert_eq!(selected(&h), Some(Extent::Unbounded), "inverted, everything past the canvas");
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::D);
    h.run();
    assert_eq!(selected(&h), None, "Ctrl+D did not deselect");
    assert!(crate::commands::availability(h.state().state(), Cmd::InvertSelection).is_err());
    // Each a state: undone, back through them.
    command(&mut h, Cmd::Undo);
    assert_eq!(selected(&h), Some(Extent::Unbounded));
    command(&mut h, Cmd::Undo);
    assert_eq!(selected(&h), Some(Extent::Within(IRect::new(0, 0, 300, 200))));
    assert_eq!(h.state().state().active().unwrap().history.redo_label(), Some("Invert selection"));
}

#[test]
fn clear_erases_what_is_selected_on_the_selected_layers() {
    use opal_core::document::{LayerContent, ObjectKind};
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 300, 200);
    let pixel = |h: &Harness<'static, OpalApp>, x: i64, y: i64| {
        let d = h.state().state().active().unwrap();
        let LayerContent::Objects(o) = &d.doc().layers[0].content else { panic!("a pixel layer") };
        let ObjectKind::Pixels(p) = &o[0].kind else { panic!("pixels") };
        opal_core::tile::pixel_to_f32(p.image.pixel(x - p.offset[0], y - p.offset[1]))[3]
    };
    // Nothing selected: refused, saying why.
    let why = crate::commands::availability(h.state().state(), Cmd::Clear).unwrap_err();
    assert!(why.contains("Select an area first"), "{why}");
    // Half of pixel 199 across is selected.
    select_rect(&mut h, [100.0, 50.0, 199.5, 150.0]);
    h.key_press(egui::Key::Delete);
    h.run();
    assert_eq!(pixel(&h, 150, 100), 0.0, "inside is not erased");
    assert_eq!(pixel(&h, 50, 100), 1.0, "outside was erased");
    assert!((pixel(&h, 199, 100) - 0.5).abs() < 0.01, "a pixel half selected is not half erased");
    assert_eq!(h.state().state().active().unwrap().history.undo_label(), Some("Clear"));
}

#[test]
fn copy_and_cut_take_what_is_selected() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 300, 200);
    h.state_mut().state_mut().clipboard = Box::new(crate::clipboard::TestClipboard::default());
    let copied = |h: &mut Harness<'static, OpalApp>| match h.state_mut().state_mut().clipboard.read_image() {
        crate::clipboard::Read::Ready(r) => r.unwrap().map(|p| (p.width, p.height)),
        crate::clipboard::Read::Later(_) => panic!("the test clipboard reads at once"),
    };
    select_rect(&mut h, [10.0, 20.0, 40.0, 30.0]);
    h.event(egui::Event::Copy);
    h.run();
    assert_eq!(copied(&mut h), Some((30, 10)), "not trimmed to the selection");
    // Cut: the same, then cleared.
    select_rect(&mut h, [50.0, 60.0, 70.0, 100.0]);
    h.event(egui::Event::Cut);
    h.run();
    assert_eq!(copied(&mut h), Some((20, 40)));
    let d = h.state().state().active().unwrap();
    assert_eq!(d.history.undo_label(), Some("Cut"));
    let flat = opal_core::composite::composite_within(&d.doc().layers, &d.doc().canvas.rect());
    assert_eq!(opal_core::tile::pixel_to_f32(flat.pixel(60, 80))[3], 0.0, "what was cut is still there");
    assert_eq!(opal_core::tile::pixel_to_f32(flat.pixel(45, 80))[3], 1.0);
}

#[test]
fn crop_to_selection_makes_the_canvas_the_rectangle_round_it() {
    use opal_core::document::Canvas;
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 300, 200);
    // Pixel 110 across is half selected, and so within it.
    select_rect(&mut h, [10.0, 20.0, 110.5, 70.0]);
    command(&mut h, Cmd::CropToSelection);
    let canvas = |h: &Harness<'static, OpalApp>| h.state().state().active().unwrap().doc().canvas;
    assert_eq!(canvas(&h), Canvas { x: 10, y: 20, width: 101, height: 50 });
    // Inverted, it reaches past every edge: nothing to crop to.
    command(&mut h, Cmd::InvertSelection);
    command(&mut h, Cmd::CropToSelection);
    assert_eq!(canvas(&h), Canvas { x: 10, y: 20, width: 101, height: 50 });
}

#[test]
fn the_selection_s_outline_shows_as_marching_ants_on_the_canvas() {
    let _gpu = gpu_lock();
    let mut h = harness();
    h.state_mut().state_mut().create_image(&crate::state::NewImage {
        width: 300,
        height: 200,
        background: crate::state::Background::Transparent,
        resolution: 72.0,
    });
    h.run();
    // The top edge of a selection, on the screen: how many of its pixels are black.
    let black_along_top = |h: &mut Harness<'static, OpalApp>| {
        let d = h.state().state().active().unwrap();
        let (view, min) = (d.view, d.view.screen_rect.min);
        let at = |x: f32, y: f32| min + view.to_screen(egui::vec2(x, y)) / h.ctx.pixels_per_point();
        let (a, b) = (at(100.0, 50.0), at(200.0, 50.0));
        let img = render(h, "ants");
        let ppp = h.ctx.pixels_per_point();
        let y = (a.y * ppp).floor() as u32;
        ((a.x * ppp) as u32 + 2..(b.x * ppp) as u32 - 2)
            .filter(|&x| {
                let p = img.get_pixel(x, y);
                p[0] < 40 && p[1] < 40 && p[2] < 40
            })
            .count()
    };
    assert_eq!(black_along_top(&mut h), 0, "black along the top with nothing selected");
    select_rect(&mut h, [100.0, 50.0, 200.0, 150.0]);
    assert!(black_along_top(&mut h) > 10, "no ants along the selection's top");
}

/// Drags with the Select tool from `from` to `to` (document pixels) on the active image,
/// `held` down as it starts and `then` once it has (and through to its end).
fn select_drag(
    h: &mut Harness<'static, OpalApp>,
    from: [f32; 2],
    to: [f32; 2],
    held: egui::Modifiers,
    then: egui::Modifiers,
) {
    h.state_mut().state_mut().set_tool(crate::tools::ToolId::Select);
    h.run();
    let v = h.state().state().active().unwrap().view;
    let ppp = h.ctx.pixels_per_point();
    let at = |[x, y]: [f32; 2]| v.screen_rect.min + v.to_screen(egui::vec2(x, y)) / ppp;
    h.event(egui::Event::ModifiersChanged(held));
    h.hover_at(at(from));
    h.run();
    h.drag_at(at(from));
    h.run();
    for k in [0.25, 0.5, 0.75] {
        h.hover_at(at([from[0] + (to[0] - from[0]) * k, from[1] + (to[1] - from[1]) * k]));
        h.run();
    }
    h.event(egui::Event::ModifiersChanged(then));
    h.hover_at(at(to));
    h.run();
    h.drop_at(at(to));
    h.run();
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    h.run();
}

#[test]
fn the_select_tool_selects_a_rectangle_dragged_on_whole_pixels_and_combines_by_the_keys() {
    use egui::Modifiers;
    use opal_core::geom::IRect;
    use opal_core::mask::Extent;
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 400, 300);
    let none = Modifiers::NONE;
    // Its corners on the pixels' corners nearest the pointer.
    select_drag(&mut h, [100.3, 50.7], [200.6, 120.2], none, none);
    assert_eq!(selected(&h), Some(Extent::Within(IRect::new(100, 51, 201, 120))));
    assert_eq!(h.state().state().active().unwrap().history.undo_label(), Some("Rectangle selection"));
    // Shift as it starts adds another.
    select_drag(&mut h, [300.0, 200.0], [350.0, 250.0], Modifiers::SHIFT, Modifiers::SHIFT);
    assert_eq!(selected(&h), Some(Extent::Within(IRect::new(100, 51, 350, 250))));
    // Alt takes away.
    select_drag(&mut h, [290.0, 190.0], [360.0, 260.0], Modifiers::ALT, Modifiers::ALT);
    assert_eq!(selected(&h), Some(Extent::Within(IRect::new(100, 51, 201, 120))));
    // Without keys, it replaces.
    select_drag(&mut h, [10.0, 10.0], [20.0, 30.0], none, none);
    assert_eq!(selected(&h), Some(Extent::Within(IRect::new(10, 10, 20, 30))));
    // Shift pressed once dragging: a square; Alt: from its middle.
    select_drag(&mut h, [100.0, 100.0], [140.0, 120.0], none, Modifiers::SHIFT);
    assert_eq!(selected(&h), Some(Extent::Within(IRect::new(100, 100, 140, 140))), "not a square");
    select_drag(&mut h, [100.0, 100.0], [140.0, 120.0], none, Modifiers::ALT);
    assert_eq!(selected(&h), Some(Extent::Within(IRect::new(60, 80, 140, 120))), "not from its middle");
    // A click selects nothing.
    let v = h.state().state().active().unwrap().view;
    let at = v.screen_rect.min + v.to_screen(egui::vec2(50.0, 50.0)) / h.ctx.pixels_per_point();
    h.hover_at(at);
    h.run();
    h.drag_at(at);
    h.drop_at(at);
    h.run();
    assert_eq!(selected(&h), None, "a click did not deselect");
}

#[test]
fn the_select_tool_s_variant_is_chosen_atop_its_settings_and_an_ellipse_drawn() {
    use crate::tools::ToolId;
    use egui_kittest::kittest::by;
    use opal_core::geom::IRect;
    use opal_core::mask::{Extent, FULL};
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 400, 300);
    h.state_mut().state_mut().set_tool(ToolId::Select);
    h.run();
    // A button a variant, not a setting listed below too.
    h.get(on_side(by().label("Ellipse").role(egui::accesskit::Role::RadioButton), false)).click();
    h.run();
    let settings = |h: &Harness<'static, OpalApp>| h.state().state().tool_settings(ToolId::Select).clone();
    assert_eq!(settings(&h).choice("select.shape"), "ellipse");
    assert_eq!(ToolId::Select.icon_with(&settings(&h)), crate::icons::ELLIPSE, "the tool's button shows the rectangle");
    assert!(h.query(on_side(by().label("Shape"), false)).is_none(), "the variant is listed as a setting too");
    select_drag(&mut h, [100.0, 100.0], [200.0, 160.0], egui::Modifiers::NONE, egui::Modifiers::NONE);
    assert_eq!(selected(&h), Some(Extent::Within(IRect::new(100, 100, 200, 160))));
    assert_eq!(h.state().state().active().unwrap().history.undo_label(), Some("Ellipse selection"));
    let level = |h: &Harness<'static, OpalApp>, x, y| {
        h.state().state().active().unwrap().pixel_selection().unwrap().mask().level_at(x, y)
    };
    assert_eq!([level(&h, 150, 130), level(&h, 101, 101)], [FULL, 0], "not an ellipse");
    // Esc partway gives the drag up.
    let (v, ppp) = (h.state().state().active().unwrap().view, h.ctx.pixels_per_point());
    let at = |x: f32, y: f32| v.screen_rect.min + v.to_screen(egui::vec2(x, y)) / ppp;
    h.hover_at(at(10.0, 10.0));
    h.run();
    h.drag_at(at(10.0, 10.0));
    h.run();
    h.hover_at(at(50.0, 50.0));
    h.run();
    h.key_press(egui::Key::Escape);
    h.run();
    h.hover_at(at(60.0, 60.0));
    h.run();
    h.drop_at(at(60.0, 60.0));
    h.run();
    assert_eq!(selected(&h), Some(Extent::Within(IRect::new(100, 100, 200, 160))), "Esc did not give the drag up");
}
