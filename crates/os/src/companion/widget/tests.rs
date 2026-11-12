use rs_lvgl::Display;

use super::*;
use crate::test_support;

/// Mount the widget on a 480x480 memory display and return the live UI.
fn mount<'a>(control: &Rc<FoxControl>, display: &'a Display) -> rs_lvgl_ui::Ui<()> {
    struct App {
        control: Rc<FoxControl>,
    }
    impl rs_lvgl_ui::App for App {
        type Action = ();
        fn update(&mut self, (): ()) {}
        fn view(&self, _theme: &rs_lvgl_ui::Theme) -> rs_lvgl_ui::View<()> {
            rs_lvgl_ui::custom(FoxWidget::new(Rc::clone(&self.control)), ())
        }
    }
    let app = App {
        control: Rc::clone(control),
    };
    let mut ui = rs_lvgl_ui::Ui::new(display, rs_lvgl_ui::Theme::dark()).expect("Ui::new");
    ui.mount(&app).expect("mount");
    ui
}

#[cfg(feature = "lua-engine")]
#[test]
fn resident_lua_selects_pose_while_rust_renderer_keeps_animating_and_expires_it() {
    let _guard = test_support::lock();
    unsafe { rs_lvgl::deinit() };
    rs_lvgl::init();
    let display = Display::register::<_, { 480 * 4 }>(480, 480, |_| {}).unwrap();
    let control = FoxControl::new(Expression::Idle);
    let mut runtime = crate::companion::runtime::Runtime::new(0, Rc::clone(&control));
    runtime.enable_brain("local first=true;return {on_event=function()end,tick=function()fox.set_expression('Happy');if first then fox.play('Charging',500);first=false end end}",0,&lua_engine::GLOBAL_ALLOCATOR,None).unwrap();
    let mut ui = mount(&control, &display);
    for now in [50, 100, 150, 200] {
        runtime.tick(now);
        ui.tick(50).unwrap();
    }
    assert_eq!(control.rendered_pose.get(), Some(FoxPose::Charging));
    assert!(
        control.rendered_frames() >= 4,
        "Rust redraws independent animation frames between Lua decisions"
    );
    runtime.tick(550);
    ui.tick(50).unwrap();
    ui.tick(60).unwrap();
    assert_eq!(control.rendered_pose.get(), Some(FoxPose::Happy));
    runtime.begin_shutdown();
    ui.tick(5).unwrap();
    ui.tick(60).unwrap();
    assert_eq!(control.rendered_pose.get(), Some(FoxPose::Sleep));
    ui.unmount();
    drop(ui);
    unsafe {
        display.delete();
        rs_lvgl::deinit();
    }
}

#[test]
fn a_scale_change_waits_for_the_cadence() {
    let _guard = test_support::lock();

    // SAFETY: the test lock is the only LVGL user in this test binary.
    unsafe { rs_lvgl::deinit() };
    rs_lvgl::init();
    let display = Display::register::<_, { 480 * 4 }>(480, 480, |_| {}).expect("display");
    let control = FoxControl::new(Expression::Idle);
    let mut ui = mount(&control, &display);
    ui.tick(50).expect("tick");
    let before = control.rendered_frames();

    let mut visual = control.visual();
    visual.scale_x = 1.05;
    control.set_visual(visual);
    ui.tick(5).expect("tick");
    assert_eq!(
        control.rendered_frames(),
        before,
        "eased gaze/scale rides the 50 ms cadence, not every loop"
    );
    ui.tick(FRAME_INTERVAL_MS).expect("tick");
    assert_eq!(
        control.rendered_frames(),
        before + 1,
        "the next animation frame draws the new shape"
    );

    ui.unmount();
    drop(ui);
    // SAFETY: the UI is unmounted and this test is the last LVGL user.
    unsafe { display.delete() };
    // SAFETY: the display is gone.
    unsafe { rs_lvgl::deinit() };
}

#[test]
fn shutdown_receipt_rejects_the_old_face_during_the_sleep_swap() {
    let _guard = test_support::lock();
    unsafe { rs_lvgl::deinit() };
    rs_lvgl::init();
    let display = Display::register::<_, { 480 * 4 }>(480, 480, |_| {}).unwrap();
    let control = FoxControl::new(Expression::Idle);
    let mut ui = mount(&control, &display);
    ui.tick(50).unwrap();
    control.set_expression(Expression::Sleep);
    let before = control.rendered_frames();
    ui.tick(5).unwrap();
    assert!(
        control.rendered_frames() > before,
        "new frame still contains the old pose during SWAP_MS"
    );
    assert_eq!(control.rendered_pose.get(), Some(FoxPose::Idle));
    assert!(!control.shutdown_sleep_frame_since(before));
    let before = control.rendered_frames();
    ui.tick(60).unwrap();
    assert_eq!(control.rendered_pose.get(), Some(FoxPose::Sleep));
    assert!(control.shutdown_sleep_frame_since(before));
    assert!(
        !control.shutdown_sleep_frame_since(control.rendered_frames()),
        "stale frame is not a receipt"
    );
    ui.unmount();
    drop(ui);
    unsafe {
        display.delete();
        rs_lvgl::deinit()
    };
}

#[test]
fn link_status_reaches_plane_frames_without_waiting_for_animation_cadence() {
    use crate::services::radio::{BluetoothState, Snapshot, WifiLink, WifiState};
    let _guard = test_support::lock();
    unsafe { rs_lvgl::deinit() };
    rs_lvgl::init();
    let display = Display::register::<_, { 480 * 4 }>(480, 480, |_| {}).unwrap();
    let control = FoxControl::new(Expression::Idle);
    control.set_plane_mode(true);
    control.set_radio_status(Snapshot::new());
    let mut ui = mount(&control, &display);
    ui.tick(50).unwrap();
    let baseline = control.take_frame().unwrap().pixels().to_vec();
    for status in [
        Snapshot {
            wifi: WifiState::Connected,
            wifi_link: WifiLink::Connected {
                rssi_dbm: Some(-55),
            },
            bluetooth: BluetoothState::Connected,
            ip: None,
        },
        Snapshot {
            wifi: WifiState::Scanning,
            wifi_link: WifiLink::Connected {
                rssi_dbm: Some(-89),
            },
            bluetooth: BluetoothState::Streaming,
            ip: None,
        },
        Snapshot {
            wifi: WifiState::Connected,
            wifi_link: WifiLink::Connected { rssi_dbm: None },
            bluetooth: BluetoothState::Discoverable,
            ip: None,
        },
        Snapshot::new(),
    ] {
        control.set_radio_status(status);
        ui.tick(0).unwrap();
        let frame = control
            .take_frame()
            .expect("link changes publish an urgent plane frame");
        for (index, (&pixel, &previous)) in frame.pixels().iter().zip(&baseline).enumerate() {
            let x = index % CANVAS_WIDTH;
            let y = index / CANVAS_WIDTH;
            let left = status::OFFSET.0 as usize;
            let top = status::OFFSET.1 as usize;
            if !(left..left + status::WIDTH).contains(&x)
                || !(top..top + status::HEIGHT).contains(&y)
            {
                assert_eq!(
                    pixel, previous,
                    "radio change must leave the character untouched at {x},{y}"
                );
            }
        }
        if status == Snapshot::new() {
            assert_eq!(
                frame.pixels(),
                baseline,
                "disconnect clears the old signal and playback indicators"
            );
        } else {
            assert_ne!(frame.pixels(), baseline);
        }
        drop(frame);
        control.set_radio_status(status);
        ui.tick(0).unwrap();
        assert!(
            control.take_frame().is_none(),
            "unchanged service state does not force another frame"
        );
    }
    ui.unmount();
    drop(ui);
    unsafe {
        display.delete();
        rs_lvgl::deinit();
    }
}

#[test]
fn animation_uploads_never_erase_connected_link_icons_between_panel_bursts() {
    use crate::services::display::{damage::FULL, simulator::MemoryPanel};
    use crate::services::radio::{BluetoothState, Snapshot, WifiLink, WifiState};
    use board_s31::pixels::{PixelFormat, PixelSource, RowChunks, MAX_DMA_BYTES};
    let _guard = test_support::lock();
    unsafe { rs_lvgl::deinit() };
    rs_lvgl::init();
    let display = Display::register::<_, { 480 * 4 }>(480, 480, |_| {}).unwrap();
    let control = FoxControl::new(Expression::Idle);
    control.set_plane_mode(true);
    let connected = Snapshot {
        wifi: WifiState::Connected,
        wifi_link: WifiLink::Connected {
            rssi_dbm: Some(-55),
        },
        bluetooth: BluetoothState::Connected,
        ip: None,
    };
    control.set_radio_status(connected);
    let mut ui = mount(&control, &display);
    let mut icons = alloc::vec![0; status::PIXELS];
    status::render(&mut icons, connected);
    // Start with an old page, but retain already visible connected icons.
    // The first complete Home frame must also remove every stale page pixel.
    let mut old_page = alloc::vec![0xa55a; fox::VIEW_PIXELS];
    let left = status::OFFSET.0 as usize;
    let top = status::OFFSET.1 as usize;
    for y in 0..status::HEIGHT {
        let at = (top + y) * CANVAS_WIDTH + left;
        old_page[at..at + status::WIDTH]
            .copy_from_slice(&icons[y * status::WIDTH..(y + 1) * status::WIDTH]);
    }
    let size = (CANVAS_WIDTH as u16, CANVAS_HEIGHT as u16);
    let mut panels: alloc::vec::Vec<_> = [PixelFormat::Rgb565Be, PixelFormat::Bgr888]
        .into_iter()
        .map(|format| {
            let mut panel = MemoryPanel::new(size, format);
            panel.upload(FULL, FULL, PixelSource::rgb565(&old_page, size));
            (format, panel)
        })
        .collect();
    let mut first_frame = None;
    let mut animated = false;
    for frame_number in 0..32 {
        ui.tick(FRAME_INTERVAL_MS).unwrap();
        let frame = control.take_frame().expect("animation publishes a frame");
        let pixels = frame.pixels();
        assert_eq!(pixels.len(), fox::VIEW_PIXELS);
        if let Some(first) = &first_frame {
            animated |= pixels != first;
        } else {
            first_frame = Some(pixels.to_vec());
        }
        let source = PixelSource::rgb565(pixels, size);
        for (format, panel) in &mut panels {
            // Use the production encoders and even-row chunk plan in each
            // panel format, checking icons after every addressed window.
            let mut uploaded = 0;
            for area in RowChunks::new(FULL, *format, MAX_DMA_BYTES) {
                uploaded += panel.upload(area, area, source).bytes;
                let visible = panel.pixels();
                for y in 0..status::HEIGHT {
                    for x in 0..status::WIDTH {
                        assert_eq!(
                            visible.rgb565_at(left + x, top + y),
                            icons[y * status::WIDTH + x],
                            "{format:?}, frame {frame_number}, window {area:?}, icon {x},{y}"
                        );
                    }
                }
            }
            assert_eq!(uploaded, fox::VIEW_PIXELS * format.bytes_per_pixel());
            assert!(panel.matches(source), "complete Home replaces the old page");
        }
    }
    assert!(
        animated,
        "the character must actually animate during the check"
    );
    ui.unmount();
    drop(ui);
    unsafe {
        display.delete();
        rs_lvgl::deinit();
    }
}

#[test]
fn native_composed_status_draws_above_the_character_and_clears_after_unmount() {
    use crate::services::radio::{BluetoothState, Snapshot, WifiLink, WifiState};
    use core::cell::RefCell;
    use core::ptr;
    let _guard = test_support::lock();
    unsafe { rs_lvgl::deinit() };
    rs_lvgl::init();
    let pixels = Rc::new(RefCell::new(alloc::vec![0u16; 480 * 480]));
    let output = Rc::clone(&pixels);
    let display = Display::register::<_, { 480 * 4 }>(480, 480, move |region| {
        let width = (region.area.x2 - region.area.x1 + 1) as usize;
        let height = (region.area.y2 - region.area.y1 + 1) as usize;
        let mut out = output.borrow_mut();
        for y in 0..height {
            for x in 0..width {
                let color = region.colors[y * width + x];
                out[(region.area.y1 as usize + y) * 480 + region.area.x1 as usize + x] =
                    (u16::from(color.r() >> 3) << 11)
                        | (u16::from(color.g() >> 2) << 5)
                        | u16::from(color.b() >> 3);
            }
        }
    })
    .unwrap();
    unsafe {
        lvgl_sys::lv_obj_set_style_bg_color(
            lvgl_sys::lv_screen_active(),
            lvgl_sys::lv_color_hex(0),
            0,
        );
    }
    let control = FoxControl::new(Expression::Idle);
    control.set_radio_status(Snapshot::new());
    let mut ui = mount(&control, &display);
    let mut previous_character = None;
    for snapshot in [
        Snapshot::new(),
        Snapshot {
            wifi: WifiState::Connected,
            wifi_link: WifiLink::Connected {
                rssi_dbm: Some(-55),
            },
            bluetooth: BluetoothState::Streaming,
            ip: None,
        },
    ] {
        control.set_radio_status(snapshot);
        ui.tick(0).unwrap();
        unsafe { lvgl_sys::lv_refr_now(ptr::null_mut()) };
        let mut expected = alloc::vec![0; status::PIXELS];
        status::render(&mut expected, snapshot);
        let out = pixels.borrow();
        let left = (480 - CANVAS_WIDTH) / 2 + status::OFFSET.0 as usize;
        let top = ((480 - CANVAS_HEIGHT) as i32 / 2 + status::OFFSET.1) as usize;
        for y in 0..fox::VIEW_ORIGIN.1 {
            for x in 0..480 {
                let expected = if (left..left + status::WIDTH).contains(&x)
                    && (top..top + status::HEIGHT).contains(&y)
                {
                    expected[(y - top) * status::WIDTH + x - left]
                } else {
                    0
                };
                assert_eq!(out[y * 480 + x], expected, "status at {x},{y}");
            }
        }
        let character = out[fox::VIEW_ORIGIN.1 * 480..].to_vec();
        if let Some(previous) = &previous_character {
            assert_eq!(
                &character, previous,
                "radio changes leave the character intact"
            );
        }
        previous_character = Some(character);
    }
    ui.unmount();
    unsafe { lvgl_sys::lv_refr_now(ptr::null_mut()) };
    assert!(
        pixels.borrow().iter().all(|&c| c == 0),
        "unmount must clear both the character and the composed icons"
    );
    drop(ui);
    unsafe {
        display.delete();
        rs_lvgl::deinit();
    }
}

#[cfg(feature = "lua-engine")]
#[test]
fn default_brain_touch_ages_after_drains_and_publishes_an_urgent_frame() {
    let _guard = test_support::lock();
    unsafe { rs_lvgl::deinit() };
    rs_lvgl::init();
    let display = Display::register::<_, { 480 * 4 }>(480, 480, |_| {}).unwrap();
    let control = FoxControl::new(Expression::Idle);
    control.set_plane_mode(true);
    let mut runtime = crate::companion::runtime::Runtime::new(0, Rc::clone(&control));
    runtime
        .enable_brain(
            crate::companion::brain::DEFAULT_SOURCE,
            0,
            &lua_engine::GLOBAL_ALLOCATOR,
            None,
        )
        .unwrap();
    let mut ui = mount(&control, &display);
    runtime.tick(1_000);
    ui.tick(50).unwrap();
    let baseline = control.take_frame().unwrap().pixels().to_vec();
    let before = control.rendered_frames();
    assert!(
        runtime.on_pointer(crate::companion::behavior::personality::PointerSample {
            at_ms: 1_000,
            x: 320,
            y: 240,
            pressed: true,
        })
    );

    // The pre-drain timestamp leaves the real default brain's pulse at zero.
    runtime.tick(1_000);
    assert_eq!(control.visual().pulse, 0.0);
    ui.tick(5).unwrap();
    assert_eq!(control.rendered_frames(), before);
    assert!(control.take_frame().is_none());

    // A fresh timestamp includes time spent draining input. The existing
    // pulse-rise policy must publish actual pixels before the 50 ms cadence.
    runtime.tick(1_020);
    assert!(control.visual().pulse > 0.0);
    assert!(control.urgent.get());
    ui.tick(5).unwrap();
    assert_eq!(control.rendered_frames(), before + 1);
    let frame = control.take_frame().expect("touch response frame");
    assert_ne!(frame.pixels(), baseline.as_slice());
    drop(frame);
    ui.unmount();
    drop(ui);
    unsafe {
        display.delete();
        rs_lvgl::deinit();
    }
}

#[test]
fn a_tap_pulse_renders_immediately() {
    let _guard = test_support::lock();

    // SAFETY: the test lock is the only LVGL user in this test binary.
    unsafe { rs_lvgl::deinit() };
    rs_lvgl::init();
    let display = Display::register::<_, { 480 * 4 }>(480, 480, |_| {}).expect("display");
    let control = FoxControl::new(Expression::Idle);
    let mut ui = mount(&control, &display);
    ui.tick(50).expect("tick");
    let before = control.rendered_frames();

    let mut visual = control.visual();
    visual.pulse = 0.5;
    control.set_visual(visual);
    ui.tick(5).expect("tick");
    assert_eq!(
        control.rendered_frames(),
        before + 1,
        "the tap thump rise must not wait for the animation cadence"
    );
    let after_rise = control.rendered_frames();

    // The decay must ride the cadence: rendering every 5 ms decay step
    // was a real device bug (P95 30 ms -> 46 ms).
    for step in 1..=5 {
        let mut visual = control.visual();
        visual.pulse = 0.5 - 0.05 * step as f32;
        control.set_visual(visual);
        ui.tick(5).expect("tick");
    }
    assert_eq!(
        control.rendered_frames(),
        after_rise,
        "pulse decay is animation and must not render every loop"
    );
    ui.tick(FRAME_INTERVAL_MS).expect("tick");
    assert_eq!(
        control.rendered_frames(),
        after_rise + 1,
        "the decay still draws on the animation cadence"
    );

    ui.unmount();
    drop(ui);
    // SAFETY: the UI is unmounted and this test is the last LVGL user.
    unsafe { display.delete() };
    // SAFETY: the display is gone.
    unsafe { rs_lvgl::deinit() };
}

#[test]
fn the_entrance_finishes_and_reports_once() {
    let _guard = test_support::lock();

    // SAFETY: the test lock is the only LVGL user in this test binary.
    unsafe { rs_lvgl::deinit() };
    rs_lvgl::init();
    let display = Display::register::<_, { 480 * 4 }>(480, 480, |_| {}).expect("display");
    let control = FoxControl::new(Expression::Idle);
    let mut ui = mount(&control, &display);
    control.start_entrance();
    assert!(control.entrance_active());

    let mut elapsed = 0u32;
    while elapsed < crate::companion::render::entrance::TOTAL_MS as u32 + 100 {
        ui.tick(50).expect("tick");
        elapsed += 50;
    }
    assert!(!control.entrance_active(), "the entrance completed");
    assert!(
        control.take_entrance_finished(),
        "the finish edge is reported"
    );
    assert!(!control.take_entrance_finished(), "the edge is one-shot");

    ui.unmount();
    drop(ui);
    // SAFETY: the UI is unmounted and this test is the last LVGL user.
    unsafe { display.delete() };
    // SAFETY: the display is gone.
    unsafe { rs_lvgl::deinit() };
}
