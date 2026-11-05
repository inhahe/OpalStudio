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
