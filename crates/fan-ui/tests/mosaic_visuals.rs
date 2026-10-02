//! RGB565 visual regression for Control Center caps and rounded-screen safety.
#![cfg(feature = "embedded")]

use std::{cell::RefCell, path::Path, rc::Rc};

use fan_ui::MosaicApp;
use slint::{
    platform::{
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel},
        Platform, PlatformError, WindowAdapter,
    },
    ComponentHandle, PhysicalSize, Rgb8Pixel,
};

struct TestPlatform(Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>);

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        window.set_size(PhysicalSize::new(480, 480));
        *self.0.borrow_mut() = Some(window.clone());
        Ok(window)
    }
}

fn frame(window: &MinimalSoftwareWindow) -> Vec<Rgb565Pixel> {
    slint::platform::update_timers_and_animations();
    window.window().request_redraw();
    let mut pixels = vec![Rgb565Pixel(0); 480 * 480];
    window.draw_if_needed(|renderer| {
        renderer.render(&mut pixels, 480);
    });
    pixels
}

fn rgb(pixels: &[Rgb565Pixel], x: usize, y: usize) -> (u8, u8, u8) {
    let p: Rgb8Pixel = pixels[y * 480 + x].into();
    (p.r, p.g, p.b)
}

fn export(name: &str, pixels: &[Rgb565Pixel]) {
    let Some(directory) = std::env::var_os("MOSAIC_VISUAL_OUTPUT") else {
        return;
    };
    let directory = Path::new(&directory);
    std::fs::create_dir_all(directory).unwrap();
    let mut output = b"P6\n480 480\n255\n".to_vec();
    for p in pixels {
        let p: Rgb8Pixel = (*p).into();
        output.extend_from_slice(&[p.r, p.g, p.b]);
    }
    std::fs::write(directory.join(format!("{name}.ppm")), output).unwrap();
}

#[test]
fn control_center_caps_and_screen_safety() {
    let created = Rc::new(RefCell::new(None));
    slint::platform::set_platform(Box::new(TestPlatform(created.clone()))).unwrap();
    let ui = MosaicApp::new().unwrap();
    ui.show().unwrap();
    let window = created.borrow().as_ref().unwrap().clone();
    ui.set_drawer_open(true);
    ui.set_screen_corner_radius(0.0);
    for level in [0, 25, 50, 75, 100] {
        ui.set_volume(level);
        ui.set_brightness(level);
        let pixels = frame(&window);
        export(&format!("drawer-level-{level}-raw"), &pixels);
        for x in [264, 368] {
            // Fill never spills into either outer corner, including at 100%.
            assert_eq!(rgb(&pixels, x, 274), (24, 24, 24));
            assert_eq!(rgb(&pixels, x + 71, 274), (24, 24, 24));
            assert_eq!(rgb(&pixels, x, 425), (24, 24, 24));
            assert_eq!(rgb(&pixels, x + 71, 425), (24, 24, 24));
            // Central samples avoid the bottom glyphs and cap antialiasing.
            let top = rgb(&pixels, x + 36, 300);
            let middle = rgb(&pixels, x + 36, 352);
            assert_eq!(
                top,
                if level == 100 {
                    (232, 236, 240)
                } else {
                    (56, 60, 56)
                }
            );
            assert_eq!(
                middle,
                if level >= 50 {
                    (232, 236, 240)
                } else {
                    (56, 60, 56)
                }
            );
        }
    }
    ui.set_volume(41);
    ui.set_brightness(58);
    ui.set_game_bricks_live(slint::ModelRc::new(slint::VecModel::from(vec![true; 24])));
    let mut scenes = vec![
        ("home".to_string(), -1, 0, false, false),
        ("apps".to_string(), -1, 1, false, false),
        ("drawer".to_string(), -1, 0, true, false),
        ("locked".to_string(), -1, 0, false, true),
    ];
    for app in 0..=8 {
        scenes.push((format!("app-{app}"), app, 0, false, false));
    }
    for (name, app, page, drawer, locked) in scenes {
        ui.set_app(app);
        ui.set_page(page);
        ui.set_drawer_open(drawer);
        ui.set_locked(locked);
        ui.set_screen_corner_radius(0.0);
        let raw = frame(&window);
        export(&format!("{name}-raw"), &raw);
        ui.set_screen_corner_radius(60.0);
        let masked = frame(&window);
        export(&format!("{name}-masked"), &masked);
        for (x, y) in [(0, 0), (479, 0), (0, 479), (479, 479)] {
            assert_eq!(rgb(&masked, x, y), (0, 0, 0), "{name} corner");
        }
        // A one-pixel margin accounts for cubic circle approximation and AA.
        // No path may remove pixels safely inside the rounded rectangle.
        for y in 0..480 {
            for x in 0..480 {
                let cx = (x as i32).clamp(60, 419);
                let cy = (y as i32).clamp(60, 419);
                let d2 = (x as i32 - cx).pow(2) + (y as i32 - cy).pow(2);
                if d2 < 58 * 58 {
                    assert_eq!(
                        raw[y * 480 + x],
                        masked[y * 480 + x],
                        "{name} visible pixel {x},{y}"
                    );
                }
            }
        }
    }
}
