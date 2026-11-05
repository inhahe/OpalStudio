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
        warnings: Vec::new(),
    }
}

fn put_on_clipboard(h: &mut Harness<'static, OpalApp>, p: crate::clipboard::Pasted) {
    h.state_mut().state_mut().clipboard =
        Box::new(crate::clipboard::TestClipboard { image: Some(p), ..Default::default() });
}

fn image_of(h: &mut Harness<'static, OpalApp>, w: u32, ht: u32) {
    h.state_mut().state_mut().create_image(&crate::state::NewImage {
        width: w,
        height: ht,
        background: crate::state::Background::White,
    });
    h.run();
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
    let expected = crate::dialogs::bytes(h.state().state().active().unwrap().history.memory());
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

/// A write that waits until the sender returned is dropped (or sent to).
fn held_write() -> (crate::writer::Job, std::sync::mpsc::Sender<()>) {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    (
        Box::new(move || {
            let _ = rx.recv();
            Ok(())
        }),
        tx,
    )
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
    let loaded = opal_io::native::load(&path).expect("the file reads back");
    assert_eq!(loaded.doc.canvas.width, 300);
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn quitting_waits_for_the_files_being_written() {
    let _gpu = gpu_lock();
    let mut h = harness();
    image_of(&mut h, 64, 64);
    let ctx = h.ctx.clone();
    let (job, release) = held_write();
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
        let (job, release) = held_write();
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
    let (first, release_first) = held_write();
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let second: crate::writer::Job = Box::new(move || {
        let _ = rx.recv();
        if fails { Err("the disk is full".into()) } else { Ok(()) }
    });
    let release_second = tx;
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
        Box::new(move || {
            let _ = rx.recv();
            if fails { Err("the disk is full".into()) } else { Ok(()) }
        }),
        tx,
    )
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
        .filter_map(|w| match w {
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
