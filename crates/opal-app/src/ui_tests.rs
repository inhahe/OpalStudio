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
    Harness::builder().with_size([1400.0, 860.0]).with_max_steps(12).wgpu().build_eframe(|cc| OpalApp::for_tests(cc))
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
fn ribbon_tabs_switch_and_commands_run() {
    let _gpu = gpu_lock();
    let mut h = harness();
    h.state_mut().state_mut().create_image(&crate::state::NewImage {
        width: 64,
        height: 64,
        background: crate::state::Background::Transparent,
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
    h.state_mut().state_mut().create_image(&crate::state::NewImage {
        width: 400,
        height: 300,
        background: crate::state::Background::White,
    });
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
        });
        // A white square left of the canvas, as a layer that extends past its edge would have.
        let d = st.active_mut().unwrap();
        let layer = d.active_layer.unwrap();
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
    h.state_mut().state_mut().create_image(&crate::state::NewImage {
        width: 32,
        height: 32,
        background: crate::state::Background::White,
    });
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
    h.state_mut().state_mut().create_image(&crate::state::NewImage { width: 64, height: 64, background });
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
    assert_ne!(h.state().state().active().unwrap().active_layer, Some(bottom));
    h.get_by_label("Background").click();
    h.run();
    assert_eq!(h.state().state().active().unwrap().active_layer, Some(bottom));
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
    let layer = h.state().state().active().unwrap().active_layer.unwrap();
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
