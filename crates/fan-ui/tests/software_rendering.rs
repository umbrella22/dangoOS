//! Exercise the legacy desktop UI fonts and RGB565 renderer without a display device.
#![cfg(not(feature = "embedded"))]
use std::{cell::RefCell, rc::Rc};

use fan_ui::{history::TemperatureHistory, FanControlUI};
use slint::{
    platform::{
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel},
        Platform, PlatformError, WindowAdapter,
    },
    ComponentHandle, ModelExt, ModelRc, PhysicalSize, Rgb8Pixel, VecModel,
};

struct TestPlatform(Rc<RefCell<Vec<Rc<MinimalSoftwareWindow>>>>);

slint::slint! {
    import { TemperatureCurve, TemperatureSample } from "../ui/temperature_curve.slint";
    import { ReferenceTemperatureCurve } from "fixtures/reference_temperature_curve.slint";
    export component CurveProbe inherits Window {
        width: 192px;
        height: 170px;
        background: white;
        in property <[TemperatureSample]> samples;
        TemperatureCurve { width: parent.width; height: parent.height; samples: root.samples; }
    }
    export component ReferenceCurveProbe inherits Window {
        width: 192px;
        height: 170px;
        background: white;
        in property <[float]> samples;
        ReferenceTemperatureCurve { width: parent.width; height: parent.height; samples: root.samples; }
    }
    export component NeutralTextProbe inherits Window {
        width: 480px;
        height: 480px;
        background: white;
        Text {
            text: "ikaros 0123456789 温度监控 风扇控制";
            font-family: "Noto Sans SC";
            font-size: 20px;
            color: black;
            horizontal-alignment: center;
            vertical-alignment: center;
        }
    }
}

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        window.set_size(PhysicalSize::new(480, 480));
        self.0.borrow_mut().push(window.clone());
        Ok(window)
    }
}

#[test]
fn swapped_rgb565_frames_match_full_rendering_including_aborted_publication() {
    let windows = Rc::new(RefCell::new(Vec::new()));
    slint::platform::set_platform(Box::new(TestPlatform(windows.clone()))).unwrap();
    let partial_ui = FanControlUI::new().unwrap();
    let reference_ui = FanControlUI::new().unwrap();
    let mut histories = [TemperatureHistory::default(), TemperatureHistory::default()];
    partial_ui.set_temp_history(histories[0].model());
    reference_ui.set_temp_history(histories[1].model());
    partial_ui.show().unwrap();
    reference_ui.show().unwrap();
    let partial = windows.borrow()[0].clone();
    let reference = windows.borrow()[1].clone();

    let mut buffers = [
        vec![Rgb565Pixel(0); 480 * 480],
        vec![Rgb565Pixel(0); 480 * 480],
    ];
    let mut full = vec![Rgb565Pixel(0); 480 * 480];
    let mut rgb888 = vec![Rgb8Pixel::default(); 480 * 480];
    let mut back = 0;
    let mut areas = Vec::new();
    let mut reset_repaint_history = true;

    // Warm both buffers, transition from splash, change disjoint controls, then
    // abandon a rendered frame and retry in the same buffer before swapping again.
    for frame in 0..14 {
        if frame == 8 {
            for history in &mut histories {
                history.set_samples(&[28.0, 31.0, 38.5]);
            }
        }
        for ui in [&partial_ui, &reference_ui] {
            ui.set_boot_progress((frame as f32 / 3.0).min(1.0));
            ui.set_booting(frame < 3);
            if frame >= 5 {
                let speed = [0, 25, 50, 75, 100, -1][(frame - 5) % 6];
                let duty = if speed == -1 { 65 } else { speed };
                ui.set_target_speed(speed);
                ui.set_fan_percentage(duty);
                ui.set_fan_rpm(300 + duty * 28);
                ui.set_fan_power(duty as f32 * 0.08);
            }
            if frame == 8 {
                ui.set_temperature(38.5);
            }
        }
        slint::platform::update_timers_and_animations();
        // A window invalidation alone must not discard the partial-render cache.
        partial_ui.window().request_redraw();
        reference_ui.window().request_redraw();
        let mut area = 0;
        partial.draw_if_needed(|renderer| {
            if reset_repaint_history {
                renderer.set_repaint_buffer_type(RepaintBufferType::NewBuffer);
                renderer.set_repaint_buffer_type(RepaintBufferType::SwappedBuffers);
                reset_repaint_history = false;
            }
            let dirty = renderer.render(&mut buffers[back], 480);
            area = dirty
                .iter()
                .map(|(_, size)| size.width as usize * size.height as usize)
                .sum();
        });
        reference.draw_if_needed(|renderer| {
            renderer.render(&mut full, 480);
            renderer.render(&mut rgb888, 480);
        });
        let mismatch = buffers[back].iter().zip(&full).position(|(a, b)| a != b);
        assert_eq!(mismatch, None, "partial frame {frame} left stale pixels");
        areas.push(area);

        if frame == 9 {
            // A timed-out publish does not swap the physical buffers. Invalidate
            // Slint's two-frame history exactly as the firmware does on abort.
            reset_repaint_history = true;
        } else {
            back = 1 - back;
        }
    }
    assert!(
        areas[6] < 480 * 480 / 2,
        "a speed change repainted {} pixels",
        areas[6]
    );
    println!("RGB565 dirty pixels per frame: {areas:?}");

    // Record the difference from RGB888, including repeated path compositing.
    let mut max_channel_error = [0u8; 3];
    for (rgb565, reference) in full.iter().zip(&rgb888) {
        let actual: Rgb8Pixel = (*rgb565).into();
        for (channel, (a, b)) in [
            (actual.r, reference.r),
            (actual.g, reference.g),
            (actual.b, reference.b),
        ]
        .into_iter()
        .enumerate()
        {
            max_channel_error[channel] = max_channel_error[channel].max(a.abs_diff(b));
        }
    }
    println!("RGB565 vs RGB888 max channel error: {max_channel_error:?}");

    if let Some(path) = std::env::var_os("FAN_RENDER_OUTPUT") {
        let mut ppm = b"P6\n480 480\n255\n".to_vec();
        for pixel in full {
            let rgb: Rgb8Pixel = pixel.into();
            ppm.extend_from_slice(&[rgb.r, rgb.g, rgb.b]);
        }
        std::fs::write(path, ppm).unwrap();
    }

    // Black glyphs on white exercise every antialias coverage without colored
    // paths underneath. RGB565 may quantize green differently; it must not
    // introduce a red/blue imbalance or a fringe larger than one 5-bit step.
    let probe_ui = NeutralTextProbe::new().unwrap();
    probe_ui.show().unwrap();
    let probe = windows.borrow()[2].clone();
    let mut neutral = vec![Rgb565Pixel(0); 480 * 480];
    probe.draw_if_needed(|renderer| {
        renderer.render(&mut neutral, 480);
    });
    let mut antialiased = 0;
    for pixel in neutral {
        let rgb: Rgb8Pixel = pixel.into();
        assert_eq!(rgb.r, rgb.b, "neutral glyph has a red/blue fringe");
        assert!(
            rgb.r.abs_diff(rgb.g) <= 8,
            "neutral glyph has a colored fringe: {rgb:?}"
        );
        antialiased += usize::from(rgb.r > 0 && rgb.r < 248);
    }
    assert!(
        antialiased > 100,
        "probe did not exercise glyph antialiasing"
    );
    println!("Neutral glyph antialias pixels checked: {antialiased}");

    verify_history_updates(&windows);
    verify_curve_appearance(&windows);
}

fn render_frame(window: &MinimalSoftwareWindow, buffer: &mut [Rgb565Pixel], width: usize) -> usize {
    window.window().request_redraw();
    let mut area = 0;
    window.draw_if_needed(|renderer| {
        area = renderer
            .render(buffer, width)
            .iter()
            .map(|(_, size)| size.width as usize * size.height as usize)
            .sum();
    });
    area
}

fn verify_history_updates(windows: &Rc<RefCell<Vec<Rc<MinimalSoftwareWindow>>>>) {
    let partial_ui = CurveProbe::new().unwrap();
    let partial = windows.borrow().last().unwrap().clone();
    partial.draw_if_needed(|renderer| {
        renderer.set_repaint_buffer_type(RepaintBufferType::SwappedBuffers);
    });
    let reference_ui = CurveProbe::new().unwrap();
    let reference = windows.borrow().last().unwrap().clone();
    let mut history = TemperatureHistory::default();
    // Both windows observe the same stable model; their rendering caches differ.
    for ui in [&partial_ui, &reference_ui] {
        ui.set_samples(ModelRc::new(history.model().map(|row| TemperatureSample {
            value: row.value,
            next_value: row.next_value,
        })));
        ui.show().unwrap();
    }
    partial.set_size(PhysicalSize::new(192, 170));
    reference.set_size(PhysicalSize::new(192, 170));
    let mut buffers = [
        vec![Rgb565Pixel(0); 192 * 170],
        vec![Rgb565Pixel(0); 192 * 170],
    ];
    let mut full = vec![Rgb565Pixel(0); 192 * 170];
    let mut back = 0;
    let mut compare = |label: &str| {
        let area = render_frame(&partial, &mut buffers[back], 192);
        render_frame(&reference, &mut full, 192);
        let difference = buffers[back].iter().zip(&full).position(|(a, b)| a != b);
        assert_eq!(
            difference, None,
            "{label}: partial frame differs from full rendering"
        );
        back = 1 - back;
        area
    };

    compare("empty history");
    for _ in 0..30 {
        history.push(28.0);
        compare("growing history");
    }
    compare("warm second buffer");
    for _ in 0..10 {
        history.push(28.0);
        assert_eq!(compare("unchanged rolling history"), 0);
    }
    let mut samples = [28.0; 30];
    samples[29] = 28.1;
    history.set_samples(&samples);
    let last_point_area = compare("last point only");
    assert!(
        last_point_area < 800,
        "last point repainted {last_point_area} pixels"
    );
    compare("last point buffer catchup");

    let mut rolling_max = 0;
    for tick in 0..100 {
        history.push(28.0 + 1.6 * (tick as f32 / 9.0).sin());
        rolling_max = rolling_max.max(compare("rolling temperature"));
    }
    assert!(rolling_max > 0, "history probe must visibly update");
    assert!(
        rolling_max < 192 * 170 / 4,
        "history repainted {rolling_max} pixels"
    );

    for values in [&[15.0, 70.0, 15.0][..], &[70.0][..], &[][..]] {
        history.set_samples(values);
        compare("shorter history");
        compare("shorter history buffer catchup");
    }
    println!("History dirty pixels: unchanged=0, last-point={last_point_area}, rolling-max={rolling_max}");
}

fn verify_curve_appearance(windows: &Rc<RefCell<Vec<Rc<MinimalSoftwareWindow>>>>) {
    let current_ui = CurveProbe::new().unwrap();
    let current = windows.borrow().last().unwrap().clone();
    let reference_ui = ReferenceCurveProbe::new().unwrap();
    let reference = windows.borrow().last().unwrap().clone();
    for window in [&current, &reference] {
        window.set_size(PhysicalSize::new(192, 170));
    }
    current_ui.show().unwrap();
    reference_ui.show().unwrap();
    let mut actual = vec![Rgb565Pixel(0); 192 * 170];
    let mut expected = actual.clone();
    let mut actual_rgb = vec![Rgb8Pixel::default(); 192 * 170];
    let mut expected_rgb = actual_rgb.clone();
    let mut changed_pixels = 0;
    let mut max_rgb_error = 0;
    let histories = [
        vec![],
        vec![28.0],
        vec![15.0, 70.0],
        vec![70.0, 15.0],
        vec![28.0; 30],
        vec![15.0; 30],
        vec![70.0; 30],
        (0..30)
            .map(|i| if i % 2 == 0 { 15.0 } else { 70.0 })
            .collect(),
        (0..30)
            .map(|i| 28.0 + 1.6 * (i as f32 / 9.0).sin())
            .collect(),
    ];
    for samples in histories {
        let rows: Vec<_> = samples
            .iter()
            .enumerate()
            .map(|(i, &value)| TemperatureSample {
                value,
                next_value: samples.get(i + 1).copied().unwrap_or(value),
            })
            .collect();
        current_ui.set_samples(ModelRc::new(VecModel::from(rows)));
        reference_ui.set_samples(ModelRc::new(VecModel::from(samples.clone())));
        render_frame(&current, &mut actual, 192);
        render_frame(&reference, &mut expected, 192);
        let differences = actual.iter().zip(&expected).filter(|(a, b)| a != b).count();
        // Translating a stroke into a smaller mask can change floating-point
        // rounding before antialiasing. Bound that error in RGB888 as well as
        // RGB565; full vs partial rendering above must remain exactly equal.
        for (window, buffer) in [(&current, &mut actual_rgb), (&reference, &mut expected_rgb)] {
            window.window().request_redraw();
            window.draw_if_needed(|renderer| {
                renderer.render(buffer, 192);
            });
        }
        let rgb_error = actual_rgb
            .iter()
            .zip(&expected_rgb)
            .flat_map(|(a, b)| [a.r.abs_diff(b.r), a.g.abs_diff(b.g), a.b.abs_diff(b.b)])
            .max()
            .unwrap_or(0);
        assert!(
            rgb_error <= 1,
            "cropping changed curve by {rgb_error} RGB888 levels for {samples:?}"
        );
        assert!(
            differences <= 8,
            "cropping changed {differences} RGB565 pixels for {samples:?}"
        );
        changed_pixels += differences;
        max_rgb_error = max_rgb_error.max(rgb_error);
    }
    println!("Curve appearance vs original: {changed_pixels} RGB565 rounding differences, max RGB888 error {max_rgb_error}/255 (empty, single, flat, limits, slopes, rolling).");
}
