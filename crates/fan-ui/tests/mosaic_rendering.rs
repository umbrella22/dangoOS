//! Exercise the actual firmware UI root, its routes, modal input, and RGB565 frames.
#![cfg(feature = "embedded")]

extern crate alloc;

#[path = "../../fan-firmware/src/quick_feedback.rs"]
mod quick_feedback;
#[path = "../../fan-firmware/src/screen_contact.rs"]
mod screen_contact;

use std::{cell::RefCell, rc::Rc};

use fan_ui::MosaicApp;
use slint::{
    platform::{
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel},
        Platform, PlatformError, PointerEventButton, WindowAdapter, WindowEvent,
    },
    ComponentHandle, LogicalPosition, PhysicalSize, Rgb8Pixel,
};

struct TestPlatform(Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>);

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        window.set_size(PhysicalSize::new(480, 480));
        *self.0.borrow_mut() = Some(window.clone());
        LATEST_WINDOW.with(|latest| *latest.borrow_mut() = Some(window.clone()));
        Ok(window)
    }
}

thread_local! {
    static LATEST_WINDOW: RefCell<Option<Rc<MinimalSoftwareWindow>>> = const { RefCell::new(None) };
    static SCREEN_CONTACT: RefCell<screen_contact::ScreenContact> = RefCell::new(screen_contact::ScreenContact::default());
    static RETAINED_FRAME: RefCell<Vec<Rgb565Pixel>> = RefCell::new(vec![Rgb565Pixel(0); 480 * 480]);
}

fn render(window: &MinimalSoftwareWindow) -> Vec<Rgb565Pixel> {
    slint::platform::update_timers_and_animations();
    window.window().request_redraw();
    RETAINED_FRAME.with(|frame| {
        let mut frame = frame.borrow_mut();
        window.draw_if_needed(|renderer| {
            renderer.render(&mut frame, 480);
        });
        frame.clone()
    })
}

fn press(window: &MinimalSoftwareWindow, x: f32, y: f32) {
    if !SCREEN_CONTACT.with(|contact| contact.borrow_mut().sample(x, y, 60.0)) {
        return;
    }
    let position = LogicalPosition::new(x, y);
    window
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    window.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
}

fn release(window: &MinimalSoftwareWindow, x: f32, y: f32) {
    if !SCREEN_CONTACT.with(|contact| contact.borrow_mut().release()) {
        render(window);
        return;
    }
    window
        .window()
        .dispatch_event(WindowEvent::PointerReleased {
            position: LogicalPosition::new(x, y),
            button: PointerEventButton::Left,
        });
    render(window);
}

fn click(window: &MinimalSoftwareWindow, x: f32, y: f32) {
    render(window);
    press(window, x, y);
    release(window, x, y);
}

fn move_pointer(window: &MinimalSoftwareWindow, x: f32, y: f32) {
    if !SCREEN_CONTACT.with(|contact| contact.borrow().accepted()) {
        return;
    }
    window.window().dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(x, y),
    });
}

fn drag(window: &MinimalSoftwareWindow, start: (f32, f32), end: (f32, f32)) {
    render(window);
    press(window, start.0, start.1);
    move_pointer(window, end.0, end.1);
    release(window, end.0, end.1);
}

fn verify_hub_and_drawer_gestures(
    ui: &MosaicApp,
    window: &MinimalSoftwareWindow,
    starts: &RefCell<i32>,
) {
    // Card origin has no TouchArea; icon origin exercises the recognizer's
    // before-children filtering and cancellation of the delayed child click.
    drag(window, (350.0, 120.0), (180.0, 125.0));
    assert_eq!(ui.get_page(), 1);
    assert_eq!(ui.get_app(), -1);
    let apps = snapshot(ui, window, "apps");
    // launcher_flow starts below the root status bar at y=56; the first icon
    // begins at y=91 and must leave this gap untouched.
    let gap: Rgb8Pixel = apps[80 * 480 + 65].into();
    assert_eq!((gap.r, gap.g, gap.b), (0, 0, 0));
    drag(window, (65.0, 156.0), (240.0, 161.0));
    assert_eq!(ui.get_page(), 0);
    assert_eq!(ui.get_app(), -1, "swipe release must not open an Apps icon");
    drag(window, (405.0, 336.0), (240.0, 336.0));
    assert_eq!(ui.get_page(), 1);
    assert_eq!(
        ui.get_app(),
        -1,
        "swipe from a Home shortcut must cancel its click"
    );
    drag(window, (65.0, 156.0), (105.0, 156.0));
    assert_eq!(ui.get_page(), 1, "short icon drag must not change page");
    assert_eq!(
        ui.get_app(),
        -1,
        "short drag must not degrade into an icon click"
    );
    drag(window, (240.0, 210.0), (100.0, 215.0));
    assert_eq!(ui.get_page(), 1, "Apps left boundary must not wrap");
    drag(window, (405.0, 156.0), (410.0, 250.0));
    assert_eq!(ui.get_page(), 1, "vertical drag must not change page");
    assert_eq!(ui.get_app(), -1);
    drag(window, (240.0, 210.0), (310.0, 290.0));
    assert_eq!(
        ui.get_page(),
        1,
        "diagonal drag without horizontal dominance must not change page"
    );
    drag(window, (240.0, 210.0), (350.0, 210.0));
    assert_eq!(ui.get_page(), 0);
    drag(window, (65.0, 336.0), (240.0, 336.0));
    assert_eq!(ui.get_page(), 0, "Home right boundary must not wrap");
    assert_eq!(
        ui.get_app(),
        -1,
        "boundary drag must cancel the shortcut click"
    );
    drag(window, (350.0, 120.0), (320.0, 120.0));
    assert_eq!(ui.get_page(), 0);
    drag(window, (350.0, 120.0), (355.0, 240.0));
    assert_eq!(ui.get_page(), 0);
    // One contact can move back and forth; it only commits its final direction once.
    press(window, 350.0, 120.0);
    move_pointer(window, 200.0, 120.0);
    assert_eq!(ui.get_page(), 0, "page must wait for release");
    move_pointer(window, 410.0, 120.0);
    move_pointer(window, 180.0, 120.0);
    release(window, 180.0, 120.0);
    assert_eq!(ui.get_page(), 1);
    assert_eq!(ui.get_app(), -1);
    render(window);
    press(window, 65.0, 156.0);
    // Allow Slint to forward the initial press to the icon before taking over.
    std::thread::sleep(std::time::Duration::from_millis(120));
    slint::platform::update_timers_and_animations();
    move_pointer(window, 240.0, 156.0);
    release(window, 240.0, 156.0);
    assert_eq!(ui.get_page(), 0);
    assert_eq!(
        ui.get_app(),
        -1,
        "a delivered icon press must still be cancelled by a swipe"
    );

    drag(window, (350.0, 420.0), (180.0, 420.0));
    assert_eq!(
        ui.get_page(),
        1,
        "bottom blank must support horizontal swipe"
    );
    drag(window, (76.0, 440.0), (240.0, 440.0));
    assert_eq!(
        ui.get_page(),
        0,
        "bottom blank must support horizontal swipe after Home removal"
    );
    drag(window, (252.0, 456.0), (120.0, 456.0));
    assert_eq!(
        ui.get_page(),
        1,
        "page dot origin must support horizontal swipe"
    );
    drag(window, (228.0, 456.0), (330.0, 456.0));
    assert_eq!(ui.get_page(), 0);
    assert_eq!(ui.get_app(), -1);

    ui.invoke_open_app(3);
    ui.set_drawer_open(true);
    render(window);
    let count = *starts.borrow();
    drag(window, (350.0, 215.0), (350.0, 185.0));
    assert!(ui.get_drawer_open(), "short upward drag must not dismiss");
    drag(window, (350.0, 100.0), (350.0, 215.0));
    assert!(ui.get_drawer_open(), "downward drag must not dismiss");
    drag(window, (350.0, 215.0), (260.0, 140.0));
    assert!(
        ui.get_drawer_open(),
        "horizontal-dominant diagonal must not dismiss"
    );
    drag(window, (350.0, 215.0), (350.0, 100.0));
    assert!(
        !ui.get_drawer_open(),
        "blank media region must support upward dismissal"
    );
    assert_eq!(ui.get_app(), 3);
    assert_eq!(
        *starts.borrow(),
        count,
        "dismissal must not start the game below"
    );
    ui.set_drawer_open(true);
    render(window);
    drag(window, (453.0, 210.0), (453.0, 28.0));
    assert!(!ui.get_drawer_open());
    assert_eq!(
        ui.get_app(),
        3,
        "dismiss release must not click the underlying exit"
    );
    ui.set_drawer_open(true);
    render(window);
    press(window, 404.0, 406.0);
    move_pointer(window, 404.0, 286.0);
    assert!(
        ui.get_brightness() >= 90,
        "slider must update before release"
    );
    release(window, 404.0, 286.0);
    assert!(
        ui.get_drawer_open(),
        "brightness upward drag belongs to its slider"
    );
    assert!(ui.get_brightness() >= 90);
    drag(window, (300.0, 406.0), (300.0, 286.0));
    assert!(
        ui.get_drawer_open(),
        "volume upward drag belongs to its slider"
    );
    assert!(ui.get_volume() >= 90);
    drag(window, (404.0, 286.0), (404.0, 406.0));
    assert!(
        ui.get_drawer_open(),
        "downward slider drag must stay on the slider"
    );
    assert!(ui.get_brightness() <= 15);
    // Slider ownership is based on the origin even after leaving its rectangle.
    drag(window, (404.0, 406.0), (355.0, 160.0));
    assert!(
        ui.get_drawer_open(),
        "a fast drag out of the slider must retain its origin"
    );
    assert_eq!(ui.get_brightness(), 100);
    press(window, 404.0, 406.0);
    move_pointer(window, 404.0, 326.0);
    move_pointer(window, 355.0, 160.0);
    release(window, 355.0, 160.0);
    assert!(ui.get_drawer_open());
    assert_eq!(ui.get_brightness(), 100);
    press(window, 300.0, 406.0);
    move_pointer(window, 300.0, 326.0);
    move_pointer(window, 355.0, 160.0);
    release(window, 355.0, 160.0);
    assert!(ui.get_drawer_open());
    assert_eq!(ui.get_volume(), 100);
    press(window, 404.0, 406.0);
    move_pointer(window, 100.0, 406.0);
    assert!(ui.get_brightness() <= 15);
    move_pointer(window, 100.0, 276.0);
    assert!(ui.get_brightness() >= 95);
    release(window, 100.0, 276.0);
    assert!(
        ui.get_drawer_open(),
        "horizontal detour must retain the original slider"
    );
    click(window, 404.0, 350.0);
    assert!(
        (49..=51).contains(&ui.get_brightness()),
        "slider tap must remain functional"
    );
    assert!(ui.get_drawer_open());
    // Starting outside and then entering the slider remains a drawer swipe.
    drag(window, (355.0, 431.0), (404.0, 276.0));
    assert!(!ui.get_drawer_open());
    assert_eq!(*starts.borrow(), count);
    // Restore the initial contract used by the existing rendering checks.
    ui.set_app(-1);
    ui.set_page(0);
    ui.set_brightness(70);
    ui.set_volume(50);
    render(window);
}

fn verify_camera_preview(ui: &MosaicApp, window: &MinimalSoftwareWindow) {
    let retries = Rc::new(RefCell::new(0));
    let retry_count = retries.clone();
    ui.on_camera_retry_requested(move || *retry_count.borrow_mut() += 1);
    ui.invoke_open_app(6);
    ui.set_camera_status("Camera initialization failed".into());
    ui.set_camera_retry_available(true);
    let unavailable = snapshot(ui, window, "camera-error-retry");
    click(window, 240.0, 312.0);
    assert_eq!(*retries.borrow(), 1);
    assert_eq!(ui.get_app(), 6);
    click(window, 36.0, 40.0);
    assert_eq!(ui.get_app(), -1, "camera error must keep its return action");
    ui.invoke_open_app(6);
    // A real square image checks propagation and cover scaling to the complete
    // preview canvas, independently of text/status comparisons.
    let mut pixels = slint::SharedPixelBuffer::<Rgb8Pixel>::new(320, 320);
    for (i, pixel) in pixels.make_mut_slice().iter_mut().enumerate() {
        let x = i % 320;
        let y = i / 320;
        *pixel = match (x < 160, y < 160) {
            (true, true) => Rgb8Pixel { r: 255, g: 0, b: 0 },
            (false, true) => Rgb8Pixel { r: 0, g: 255, b: 0 },
            (true, false) => Rgb8Pixel { r: 0, g: 0, b: 255 },
            (false, false) => Rgb8Pixel {
                r: 255,
                g: 255,
                b: 0,
            },
        };
    }
    ui.set_camera_frame(slint::Image::from_rgb8(pixels));
    let withheld = render(window);
    assert_eq!(
        withheld, unavailable,
        "a frame must stay hidden until preview-ready"
    );
    ui.set_camera_preview_ready(true);
    let preview = snapshot(ui, window, "camera-preview-fixture");
    for (x, y, rgb) in [
        (20, 100, (248, 0, 0)),
        (460, 100, (0, 252, 0)),
        (20, 350, (0, 0, 248)),
        (460, 350, (248, 252, 0)),
    ] {
        let actual: Rgb8Pixel = preview[y * 480 + x].into();
        assert_eq!((actual.r, actual.g, actual.b), rgb, "preview at {x},{y}");
    }
    // Saving is not implemented: neither the old shutter geometry nor the
    // hidden Retry geometry may invoke an action while preview is displayed.
    click(window, 240.0, 430.0);
    click(window, 240.0, 312.0);
    assert_eq!(*retries.borrow(), 1);
    assert_eq!(ui.get_app(), 6);
    ui.set_camera_preview_ready(false);
    ui.set_camera_retry_available(false);
    snapshot(ui, window, "camera-no-retry");
    click(window, 240.0, 312.0);
    assert_eq!(
        *retries.borrow(),
        1,
        "retry-available must control the action"
    );
    click(window, 36.0, 40.0);
    assert_eq!(ui.get_app(), -1);
    ui.set_camera_frame(slint::Image::default());
    ui.set_camera_status("Camera preview unavailable".into());
    render(window);
}

fn verify_control_center_routes(
    ui: &MosaicApp,
    window: &MinimalSoftwareWindow,
    starts: &RefCell<i32>,
) {
    quick_feedback::install(ui, std::time::Duration::from_millis(300));
    ui.set_app(3);
    ui.set_drawer_open(true);
    let before = *starts.borrow();
    let no_feedback = render(window);
    for (x, y, label) in [
        (76.0, 92.0, "Wi-Fi unavailable"),
        (180.0, 92.0, "Join not supported"),
        (76.0, 176.0, "Bluetooth unavailable"),
        (180.0, 176.0, "Low Power unavailable"),
        (76.0, 260.0, "Ringtone unavailable"),
        (180.0, 260.0, "Vibration unavailable"),
    ] {
        click(window, x, y);
        assert!(ui.get_drawer_open());
        assert_eq!(ui.get_app(), 3);
        assert_eq!(ui.get_quick_feedback().as_str(), label);
        assert!(!ui.get_wlan_on());
        assert!(!ui.get_bt_on());
        let feedback = snapshot(
            ui,
            window,
            &format!("feedback-{}", label.split(' ').next().unwrap()),
        );
        assert_ne!(feedback, no_feedback, "button must visibly respond");
    }
    assert_eq!(
        *starts.borrow(),
        before,
        "feedback must not trigger covered game"
    );
    click(window, 76.0, 92.0);
    std::thread::sleep(std::time::Duration::from_millis(170));
    slint::platform::update_timers_and_animations();
    click(window, 76.0, 92.0);
    std::thread::sleep(std::time::Duration::from_millis(170));
    slint::platform::update_timers_and_animations();
    assert_eq!(
        ui.get_quick_feedback().as_str(),
        "Wi-Fi unavailable",
        "repeat must restart single expiry"
    );
    click(window, 240.0, 450.0);
    assert!(!ui.get_drawer_open());
    ui.set_drawer_open(true);
    click(window, 76.0, 176.0);
    std::thread::sleep(std::time::Duration::from_millis(170));
    slint::platform::update_timers_and_animations();
    assert_eq!(
        ui.get_quick_feedback().as_str(),
        "Bluetooth unavailable",
        "old drawer expiry must not clear new feedback"
    );
    std::thread::sleep(std::time::Duration::from_millis(150));
    slint::platform::update_timers_and_animations();
    assert!(ui.get_quick_feedback().is_empty());
    click(window, 350.0, 140.0);
    assert_eq!(ui.get_app(), 7);
    assert_eq!(ui.get_status_feature(), 6);
    assert!(!ui.get_drawer_open());
    snapshot(ui, window, "music-unavailable");
    click(window, 56.0, 48.0);
    assert_eq!(ui.get_app(), 3);
    assert!(ui.get_drawer_open());
    for (x, side) in [(76.0, 0), (180.0, 1)] {
        click(window, x, 390.0);
        assert_eq!(ui.get_app(), 8);
        assert_eq!(ui.get_slot_side(), side);
        assert!(!ui.get_drawer_open());
        snapshot(
            ui,
            window,
            if side == 0 {
                "left-slot-not-checked"
            } else {
                "right-slot-not-checked"
            },
        );
        if side == 1 {
            click(window, 240.0, 196.0);
            assert_eq!(
                ui.get_app(),
                8,
                "right slot must not launch left-only camera"
            );
        }
        for (y, feature) in [(268.0, 8), (340.0, 9)] {
            click(window, 240.0, y);
            assert_eq!(ui.get_app(), 7);
            assert_eq!(ui.get_status_feature(), feature);
            snapshot(ui, window, &format!("slot-{side}-unsupported-{feature}"));
            click(window, 56.0, 48.0);
            assert_eq!(ui.get_app(), 8);
        }
        click(window, 240.0, 412.0);
        assert_eq!(ui.get_app(), 1, "slot IMU entry opens board sensor test");
        click(window, 40.0, 40.0);
        assert_eq!(ui.get_app(), 8);
        click(window, 56.0, 48.0);
        assert_eq!(ui.get_app(), 3);
        assert!(ui.get_drawer_open());
    }
    // Full source restoration including Camera capture lease, then normal Home route.
    ui.set_drawer_open(false);
    ui.set_app(0);
    ui.set_page(1);
    click(window, 240.0, 270.0);
    assert!(ui.get_drawer_open());
    click(window, 76.0, 390.0);
    assert_eq!(ui.get_app(), 8);
    click(window, 240.0, 196.0);
    assert_eq!(ui.get_app(), 6);
    snapshot(ui, window, "slot-camera");
    click(window, 36.0, 40.0);
    assert_eq!(ui.get_app(), 8);
    assert!(!ui.get_slot_test_open());
    click(window, 56.0, 48.0);
    assert_eq!(ui.get_app(), 0);
    assert!(ui.get_drawer_open());
    click(window, 240.0, 450.0);
    assert_eq!(ui.get_app(), 0);
    assert!(!ui.get_drawer_open());
    click(window, 240.0, 350.0);
    assert_eq!(ui.get_app(), 7);
    assert_eq!(ui.get_status_feature(), 0);
    snapshot(ui, window, "settings-wlan-unavailable");
    click(window, 56.0, 48.0);
    assert_eq!(ui.get_app(), 0);
    click(window, 240.0, 424.0);
    assert_eq!(ui.get_app(), 7);
    assert_eq!(ui.get_status_feature(), 7);
    snapshot(ui, window, "about");
    click(window, 56.0, 48.0);
    assert_eq!(ui.get_app(), 0);
    snapshot(ui, window, "settings-unmeasured");
    click(window, 40.0, 40.0);
    assert_eq!(ui.get_app(), -1);
    click(window, 228.0, 456.0);
    assert_eq!(ui.get_page(), 0);
    click(window, 65.0, 346.0);
    assert_eq!(ui.get_app(), 6);
    assert!(!ui.get_slot_test_open());
    click(window, 36.0, 40.0);
    assert_eq!(ui.get_app(), -1, "normal Home camera back must stay normal");
    click(window, 350.0, 150.0);
    assert_eq!(
        ui.get_app(),
        5,
        "official weather card click must launch Weather"
    );
    click(window, 40.0, 40.0);
    assert_eq!(ui.get_app(), -1);
    drag(window, (350.0, 150.0), (180.0, 150.0));
    assert_eq!(ui.get_page(), 1);
    assert_eq!(ui.get_app(), -1);
    click(window, 228.0, 456.0);
    assert_eq!(ui.get_page(), 0);
    click(window, 76.0, 456.0);
    assert_eq!(
        ui.get_app(),
        -1,
        "removed Home location is not a hidden button"
    );
    assert_eq!(*starts.borrow(), before);
}

fn verify_corner_contacts(ui: &MosaicApp, window: &MinimalSoftwareWindow) {
    ui.invoke_open_app(6);
    press(window, 5.0, 5.0);
    SCREEN_CONTACT.with(|contact| contact.borrow().transport_error());
    move_pointer(window, 36.0, 40.0);
    release(window, 36.0, 40.0);
    assert_eq!(
        ui.get_app(),
        6,
        "corner origin must not acquire a return control after moving inside"
    );
    click(window, 36.0, 40.0);
    assert_eq!(ui.get_app(), -1, "Camera back inside radius remains usable");
    ui.set_page(0);
    click(window, 5.0, 5.0);
    assert!(!ui.get_drawer_open());
    ui.set_drawer_open(true);
    press(window, 404.0, 350.0);
    move_pointer(window, 479.0, 479.0);
    release(window, 479.0, 479.0);
    assert!(ui.get_drawer_open());
    assert_eq!(
        ui.get_brightness(),
        0,
        "accepted slider contact keeps ownership outside radius"
    );
    click(window, 240.0, 450.0);
    assert!(!ui.get_drawer_open());
    ui.set_brightness(70);
    ui.set_app(-1);
    ui.set_page(0);
}

fn snapshot(ui: &MosaicApp, window: &MinimalSoftwareWindow, name: &str) -> Vec<Rgb565Pixel> {
    let frame = render(window);
    // A separately instantiated root provides a full render reference while the
    // primary root retains its framebuffer and dirty-region history across events.
    let reference_ui = MosaicApp::new().unwrap();
    reference_ui.set_page(ui.get_page());
    reference_ui.set_app(ui.get_app());
    reference_ui.set_status_feature(ui.get_status_feature());
    reference_ui.set_status_origin(ui.get_status_origin());
    reference_ui.set_slot_side(ui.get_slot_side());
    reference_ui.set_drawer_source_app(ui.get_drawer_source_app());
    reference_ui.set_slot_test_open(ui.get_slot_test_open());
    reference_ui.set_quick_feedback(ui.get_quick_feedback());
    reference_ui.set_screen_corner_radius(ui.get_screen_corner_radius());
    reference_ui.set_fan_available(ui.get_fan_available());
    reference_ui.set_fan_rpm_valid(ui.get_fan_rpm_valid());
    reference_ui.set_fan_temperature_valid(ui.get_fan_temperature_valid());

    reference_ui.set_locked(ui.get_locked());
    reference_ui.set_drawer_open(ui.get_drawer_open());
    reference_ui.set_brightness(ui.get_brightness());
    reference_ui.set_volume(ui.get_volume());
    reference_ui.set_tof_threshold(ui.get_tof_threshold());
    reference_ui.set_imu_valid(ui.get_imu_valid());
    reference_ui.set_game_bricks_live(ui.get_game_bricks_live());
    reference_ui.set_weather_art(ui.get_weather_art());
    reference_ui.set_weather_date(ui.get_weather_date());
    reference_ui.set_weather_low(ui.get_weather_low());
    reference_ui.set_weather_high(ui.get_weather_high());
    reference_ui.set_camera_frame(ui.get_camera_frame());
    reference_ui.set_camera_preview_ready(ui.get_camera_preview_ready());
    reference_ui.set_camera_status(ui.get_camera_status());
    reference_ui.set_camera_retry_available(ui.get_camera_retry_available());
    reference_ui.show().unwrap();
    let mut full = vec![Rgb565Pixel(0); 480 * 480];
    // TestPlatform exposes each adapter via its trait-independent thread-local slot.
    LATEST_WINDOW.with(|latest| {
        latest
            .borrow()
            .as_ref()
            .unwrap()
            .draw_if_needed(|renderer| {
                renderer.set_repaint_buffer_type(RepaintBufferType::NewBuffer);
                renderer.render(&mut full, 480);
            });
    });
    assert_eq!(
        frame, full,
        "retained rendering left stale pixels in {name}"
    );
    if let Some(directory) = std::env::var_os("MOSAIC_RENDER_OUTPUT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        let mut ppm = b"P6\n480 480\n255\n".to_vec();
        for pixel in &frame {
            let rgb: Rgb8Pixel = (*pixel).into();
            ppm.extend_from_slice(&[rgb.r, rgb.g, rgb.b]);
        }
        std::fs::write(directory.join(format!("{name}.ppm")), ppm).unwrap();
    }
    frame
}

#[test]
fn mosaic_routes_modals_sliders_and_imu_rendering() {
    let created = Rc::new(RefCell::new(None));
    slint::platform::set_platform(Box::new(TestPlatform(created.clone()))).unwrap();
    let ui = MosaicApp::new().unwrap();
    ui.show().unwrap();
    let window = created.borrow().as_ref().unwrap().clone();
    let starts = Rc::new(RefCell::new(0));
    let start_counter = starts.clone();
    ui.on_game_start(move || *start_counter.borrow_mut() += 1);
    let brightness = Rc::new(RefCell::new(None));
    let brightness_callback = brightness.clone();
    ui.on_brightness_changed(move |value| *brightness_callback.borrow_mut() = Some(value));

    verify_hub_and_drawer_gestures(&ui, &window, &starts);
    verify_camera_preview(&ui, &window);
    verify_control_center_routes(&ui, &window, &starts);
    verify_corner_contacts(&ui, &window);
    ui.set_game_bricks_live(slint::ModelRc::new(slint::VecModel::from(vec![true; 24])));
    let home = snapshot(&ui, &window, "home");
    // The static 10:10 hands retain their original 169×169 coordinate system;
    // automatic Path bounding-box scaling would inflate them into a large X.
    let mut hand_pixels = 0;
    for y in 98..242 {
        for x in 287..432 {
            let color: Rgb8Pixel = home[y * 480 + x].into();
            hand_pixels += usize::from(color.r == 32 && color.g == 32 && color.b == 32);
        }
    }
    assert!(
        (150..600).contains(&hand_pixels),
        "static hands painted {hand_pixels} pixels"
    );
    // Every Apps slot must work without any platform navigation callback.
    for (x, y, app) in [
        (65.0, 156.0, 0),
        (240.0, 156.0, 1),
        (405.0, 156.0, 2),
        (65.0, 346.0, 4),
        (240.0, 346.0, 3),
        (405.0, 346.0, 5),
    ] {
        ui.set_page(1);
        ui.set_app(-1);
        click(&window, x, y);
        assert_eq!(ui.get_app(), app, "Apps slot at {x},{y}");
        assert_eq!(ui.get_in_settings(), app == 0);
        snapshot(&ui, &window, &format!("app-{app}"));
        // Clicking a hidden page dot must never navigate behind an application.
        click(&window, 230.0, 474.0);
        assert_eq!(ui.get_app(), app);
        assert_eq!(ui.get_page(), 1);
        let (back_x, back_y) = if app == 3 {
            (453.0, 28.0)
        } else {
            (40.0, 40.0)
        };
        click(&window, back_x, back_y);
        assert_eq!(ui.get_app(), -1, "back from app {app}");
    }
    click(&window, 228.0, 456.0);
    assert_eq!(ui.get_page(), 0);
    assert_eq!(ui.get_app(), -1);
    for (x, app) in [(65.0, 6), (240.0, 5), (405.0, 2)] {
        click(&window, x, 346.0);
        assert_eq!(ui.get_app(), app, "Home shortcut at {x}");
        snapshot(&ui, &window, &format!("shortcut-{app}"));
        click(&window, 40.0, 40.0);
        assert_eq!(ui.get_app(), -1);
    }

    // Hub top bar opens the drawer and its handle closes it.
    click(&window, 240.0, 16.0);
    assert!(ui.get_drawer_open());
    let drawer = snapshot(&ui, &window, "drawer");
    // Connectivity icon assets must stay inside 72px tiles, leaving the 20px
    // row gaps untouched instead of spilling their native 118px circles.
    for y in 138..150 {
        for x in 34..210 {
            let color: Rgb8Pixel = drawer[y * 480 + x].into();
            assert!(
                color.r < 80 && color.g < 80 && color.b < 80,
                "drawer icon overflow at {x},{y}: {color:?}"
            );
        }
    }
    click(&window, 240.0, 450.0);
    assert!(!ui.get_drawer_open());
    snapshot(&ui, &window, "drawer-dismissed");
    press(&window, 240.0, 16.0);
    window.window().dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(240.0, 90.0),
    });
    release(&window, 240.0, 90.0);
    assert!(
        ui.get_drawer_open(),
        "a downward swipe from the top must open the drawer"
    );
    click(&window, 240.0, 450.0);

    ui.invoke_open_app(0);
    click(&window, 240.0, 270.0);
    assert!(
        ui.get_drawer_open(),
        "Settings brightness must open its slider"
    );
    // Blank top bar must consume the underlying Settings back button.
    click(&window, 40.0, 30.0);
    assert_eq!(ui.get_app(), 0);
    click(&window, 404.0, 312.0);
    assert!((74..=76).contains(&ui.get_brightness()));
    assert_eq!(*brightness.borrow(), Some(ui.get_brightness()));
    // A hover without a pressed pointer must not change brightness.
    window.window().dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(404.0, 406.0),
    });
    assert!((74..=76).contains(&ui.get_brightness()));
    click(&window, 240.0, 450.0);
    assert_eq!(ui.get_app(), 0);
    click(&window, 40.0, 30.0);
    assert_eq!(ui.get_app(), -1);

    ui.invoke_open_app(3);
    ui.set_drawer_open(true);
    let before = *starts.borrow();
    click(&window, 240.0, 215.0); // Blank media region over TAP TO START.
    assert_eq!(*starts.borrow(), before);
    click(&window, 453.0, 28.0); // Blank top bar over game exit.
    assert_eq!(ui.get_app(), 3);
    ui.set_locked(true);
    snapshot(&ui, &window, "locked");
    click(&window, 240.0, 240.0);
    assert!(ui.get_locked(), "a lock-screen tap must not unlock");
    assert_eq!(*starts.borrow(), before);
    click(&window, 453.0, 28.0);
    assert_eq!(ui.get_app(), 3);
    press(&window, 240.0, 420.0);
    window.window().dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(240.0, 390.0),
    });
    assert!(ui.get_locked(), "a short movement must not unlock");
    window.window().dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(240.0, 340.0),
    });
    release(&window, 240.0, 340.0);
    assert!(!ui.get_locked(), "an upward swipe must unlock");
    assert!(ui.get_drawer_open());
    assert_eq!(*starts.borrow(), before, "unlock must not click the game");
    snapshot(&ui, &window, "unlocked-drawer");
    ui.set_drawer_open(false);
    click(&window, 240.0, 250.0);
    assert_eq!(*starts.borrow(), before + 1);
    click(&window, 453.0, 28.0);
    assert_eq!(ui.get_app(), -1);

    ui.invoke_open_app(4);
    for (x, expected) in [
        (28.0, 0),
        (131.5, 25),
        (235.0, 50),
        (338.5, 75),
        (441.9, 100),
    ] {
        click(&window, x, 374.0);
        assert!((ui.get_tof_threshold() - expected).abs() <= 1, "TOF at {x}");
    }
    press(&window, 240.0, 374.0);
    window.window().dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(460.0, 374.0),
    });
    release(&window, 460.0, 374.0);
    assert_eq!(ui.get_tof_threshold(), 100);
    click(&window, 40.0, 40.0);
    snapshot(&ui, &window, "returned-home");
    ui.invoke_open_app(5);
    let unavailable = render(&window);
    ui.set_weather_art(1);
    ui.set_weather_date("09/30".into());
    ui.set_weather_low("18".into());
    ui.set_weather_high("26".into());
    let available = snapshot(&ui, &window, "weather-data");
    assert_ne!(
        unavailable, available,
        "weather root properties must reach the page"
    );
    ui.set_app(1);
    let invalid = snapshot(&ui, &window, "imu-invalid");
    ui.set_imu_valid(true);
    let valid = snapshot(&ui, &window, "imu-centered");
    assert_ne!(invalid, valid);
    let mut amber = Vec::new();
    for y in 74..335 {
        for x in 12..468 {
            let color: Rgb8Pixel = valid[y * 480 + x].into();
            if color.r > 245 && (165..190).contains(&color.g) && color.b < 50 {
                amber.push((x as f32, y as f32));
            }
        }
    }
    assert!(amber.len() > 2500, "valid IMU must show its amber bubble");
    let center_x = amber.iter().map(|p| p.0).sum::<f32>() / amber.len() as f32;
    let center_y = amber.iter().map(|p| p.1).sum::<f32>() / amber.len() as f32;
    assert!((center_x - 240.0).abs() < 2.0, "bubble x = {center_x}");
    assert!((center_y - 204.5).abs() < 2.0, "bubble y = {center_y}");
}
