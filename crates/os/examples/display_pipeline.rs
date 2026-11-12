//! Native LVGL shell + production display port, replayed into a host memory panel.
//! cargo sim-display -- target/display-rgb888-20261112/simulator-rgb888
use board_s31::{geometry::DirtyRect, pixels::PANEL_FORMAT};
use os::{
    services::display::{
        damage::Damage,
        lvgl_port::{self, DisplayPort},
        simulator::{MemoryPanel, Upload},
    },
    shell::{
        app::theme,
        components::splash::SplashControl,
        navigation::{
            route::{AppId, Page},
            transition::{self, Direction},
        },
        pages::settings::model::Page as SettingsPage,
    },
    AppMsg, Expression, FoxControl, MosaicoApp,
};
use rs_lvgl::{sys, Display};
use rs_lvgl_ui::Ui;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

unsafe extern "C" {
    static lv_slider_class: sys::lv_obj_class_t;
}

unsafe fn find_slider(obj: *mut sys::lv_obj_t) -> Option<*mut sys::lv_obj_t> {
    if unsafe { sys::lv_obj_has_class(obj, &raw const lv_slider_class) } {
        return Some(obj);
    }
    for index in 0..unsafe { sys::lv_obj_get_child_count(obj) } {
        let child = unsafe { sys::lv_obj_get_child(obj, index as i32) };
        if let Some(slider) = unsafe { find_slider(child) } {
            return Some(slider);
        }
    }
    None
}

struct Replay {
    panel: MemoryPanel,
    out: PathBuf,
    frames: Vec<Value>,
    comparisons: usize,
}

impl Replay {
    fn new(out: PathBuf) -> Self {
        fs::create_dir_all(&out).unwrap();
        Self {
            panel: MemoryPanel::new((480, 480), PANEL_FORMAT),
            out,
            frames: Vec::new(),
            comparisons: 0,
        }
    }

    fn save(
        &mut self,
        scene: &str,
        event: Value,
        stats: Upload,
        regions: &[DirtyRect],
        host_us: Option<u128>,
    ) {
        let file = format!("{:03}-{scene}.ppm", self.frames.len());
        fs::write(self.out.join(&file), self.panel.ppm()).unwrap();
        self.frames
            .push(json!({"file":file,"scene":scene,"event":event,
            "host_render_us":host_us,"bytes":stats.bytes,"direct_bytes":stats.direct_bytes,
            "windows":stats.windows,"wire_only_us_at_40mhz":stats.bytes as f64/20.0,
            "damage":regions.iter().map(|r|[r.x0,r.y0,r.x1,r.y1]).collect::<Vec<_>>() }));
    }

    fn present(
        &mut self,
        port: &mut DisplayPort,
        scene: &str,
        event: Value,
        host_us: u128,
    ) -> bool {
        let Some((lease, damage)) = port.take_frame() else {
            return false;
        };
        let mut stats = Upload::default();
        for region in damage.regions() {
            let s = self.panel.upload(*region, *region, lease.pixels());
            stats.bytes += s.bytes;
            stats.direct_bytes += s.direct_bytes;
            stats.windows += s.windows;
        }
        assert!(
            self.panel.matches(lease.pixels()),
            "incremental panel differs from complete shadow at {scene}"
        );
        self.comparisons += 1;
        self.save(scene, event, stats, damage.regions(), Some(host_us));
        true
    }
}

fn render(ui: &mut Ui<AppMsg>, app: &mut MosaicoApp, port: &mut DisplayPort) -> u128 {
    let started = Instant::now();
    rs_lvgl::tick_inc(std::time::Duration::from_millis(16));
    port.poll_pointer();
    ui.drain(app, 32).unwrap();
    ui.tick(16).unwrap();
    ui.timer_handler();
    port.render_now();
    started.elapsed().as_micros()
}

fn drag(
    ui: &mut Ui<AppMsg>,
    app: &mut MosaicoApp,
    port: &mut DisplayPort,
    replay: &mut Replay,
    scene: &str,
) {
    let slider =
        unsafe { find_slider(sys::lv_screen_active()) }.expect("production settings slider");
    let mut area = sys::lv_area_t {
        x1: 0,
        y1: 0,
        x2: 0,
        y2: 0,
    };
    unsafe { sys::lv_obj_get_coords(slider, &mut area) };
    let y = (area.y1 + area.y2) / 2;
    let mut values = Vec::new();
    for percent in (10i16..=90).step_by(4).chain((10i16..=90).step_by(4).rev()) {
        let percent = i32::from(percent);
        let x = area.x1 + (area.x2 - area.x1) * percent / 100;
        lvgl_port::set_pointer(x, y, true);
        let elapsed = render(ui, app, port);
        let actual = unsafe { sys::lv_slider_get_value(slider) };
        values.push(actual);
        replay.present(
            port,
            scene,
            json!({"pointer":[x,y],"pressed":true,"native_slider_value":actual}),
            elapsed,
        );
    }
    let x = area.x1 + (area.x2 - area.x1) / 10;
    lvgl_port::set_pointer(x, y, false);
    let elapsed = render(ui, app, port);
    replay.present(port, scene, json!({"pressed":false}), elapsed);
    assert!(
        values.iter().max().unwrap() - values.iter().min().unwrap() > 50,
        "pointer drag did not move slider: {values:?}"
    );
    assert_eq!(
        unsafe { sys::lv_indev_get_state(sys::lv_indev_get_next(core::ptr::null_mut())) },
        sys::lv_indev_state_t_LV_INDEV_STATE_RELEASED
    );
    // Finish native pressed/released style animations before checking a static page.
    for _ in 0..20 {
        let elapsed = render(ui, app, port);
        replay.present(port, scene, json!({"settling":true}), elapsed);
    }
    // Static scenes must not generate another pixel submission.
    let mut idle_submits = 0;
    for _ in 0..10 {
        let elapsed = render(ui, app, port);
        if replay.present(port, scene, json!({"idle":true}), elapsed) {
            idle_submits += 1;
        }
    }
    assert_eq!(idle_submits, 0, "unchanged settings page generated pixels");
}

fn run(out: &Path) {
    let mut replay = Replay::new(out.to_path_buf());
    let mut port = unsafe { DisplayPort::new() };
    let display = Display::default();
    let mut app = MosaicoApp::new(FoxControl::new(Expression::Idle), SplashControl::new());
    app.go(Page::App(AppId::Settings));
    app.set_settings_page(SettingsPage::Audio);
    let mut ui = Ui::new(&display, theme()).unwrap();
    ui.mount(&app).unwrap();
    let elapsed = render(&mut ui, &mut app, &mut port);
    assert!(replay.present(&mut port, "audio", json!({"enter":true}), elapsed));
    drag(&mut ui, &mut app, &mut port, &mut replay, "audio");

    assert!(port.snapshot());
    app.go(Page::Launcher);
    ui.mark_dirty();
    render(&mut ui, &mut app, &mut port);
    let (new, old) = port.take_slide().unwrap();
    for ms in (0..=280).step_by(20) {
        let offset = transition::offset(ms, 480);
        // Same row-major composition the plane uploads on the device.
        let stats = replay.panel.upload_rows(transition::rows(
            new.pixels(),
            old.pixels(),
            Direction::Back,
            offset,
        ));
        assert_eq!(stats.bytes, 480 * 480 * PANEL_FORMAT.bytes_per_pixel());
        // Independent pixel oracle for every intermediate push frame. It does
        // not use transition::bands or the transport's row-chunk iterator.
        for y in 0..480 {
            for x in 0..480 {
                let expected = if x < offset as usize {
                    new.pixels().rgb888_at(x + 480 - offset as usize, y)
                } else {
                    old.pixels().rgb888_at(x - offset as usize, y)
                };
                assert_eq!(
                    replay.panel.pixels().rgb888_at(x, y),
                    expected,
                    "push offset={offset} pixel=({x},{y})"
                );
            }
        }
        replay.comparisons += 1;
        replay.save(
            "push",
            json!({"offset":offset,"animation_ms":ms}),
            stats,
            Damage::full().regions(),
            None,
        );
    }
    assert!(replay.panel.matches(new.pixels()));
    drop(new);
    drop(old);
    drop(port.take_full_frame());

    app.go(Page::App(AppId::Settings));
    app.set_settings_page(SettingsPage::Display);
    ui.mark_dirty();
    let elapsed = render(&mut ui, &mut app, &mut port);
    assert!(replay.present(&mut port, "brightness", json!({"enter":true}), elapsed));
    drag(&mut ui, &mut app, &mut port, &mut replay, "brightness");
    let report = json!({"schema":1,"backend":"native LVGL 9.6 with CPU memory panel",
        "format":format!("{:?}",PANEL_FORMAT),"size":[480,480],"device_measurements":false,
        "wire_model":"40MHz quad SDR pixel bytes only; no commands, TE, DMA or optical timing",
        "full_frame_comparisons":replay.comparisons,"frames":replay.frames});
    fs::write(
        out.join("replay.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    println!(
        "{}: {} frames, {} incremental/full-frame comparisons passed",
        out.display(),
        replay.frames.len(),
        replay.comparisons
    );
    drop(ui);
    drop(app);
    drop(display);
    drop(port);
}

fn main() {
    let out = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/display-simulator"));
    rs_lvgl::init();
    run(&out);
    unsafe { rs_lvgl::deinit() };
}
