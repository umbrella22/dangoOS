//! Opt-in physical display experiments. No test controls enter normal builds.
//! Synthetic trajectories use the ordinary touch queue and retain their origin
//! in the log; they are not measurements of physical controller latency.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Status,
    Page(u8),
    Pattern(u8),
    Tone(u8),
    Swipe {
        from: (u16, u16),
        to: (u16, u16),
        ms: u16,
    },
}

impl Command {
    pub fn parse(args: &[&str]) -> Result<Self, &'static str> {
        match args {
            ["status"] => Ok(Self::Status),
            ["tone", action] => Ok(Self::Tone(match *action {
                "prepare" => 0,
                "play" => 1,
                "clean" => 2,
                _ => return Err("display-test tone prepare|play|clean"),
            })),
            ["page", page] => Ok(Self::Page(match *page {
                "home" => 0,
                "launcher" => 1,
                "settings" => 2,
                _ => return Err("display-test page home|launcher|settings"),
            })),
            ["pattern", pattern] => Ok(Self::Pattern(match *pattern {
                "off" => 0,
                "colors" => 1,
                "gradient" => 2,
                _ => return Err("display-test pattern off|colors|gradient"),
            })),
            ["swipe", x0, y0, x1, y1, ms] => {
                let values = [x0, y0, x1, y1, ms].map(|s| s.parse::<u16>());
                let [Ok(x0), Ok(y0), Ok(x1), Ok(y1), Ok(ms)] = values else {
                    return Err("display-test invalid integer");
                };
                if x0 >= 480 || x1 >= 480 || y0 >= 480 || y1 >= 480 || !(20..=2000).contains(&ms) {
                    return Err("display-test coordinates 0..479, duration 20..2000 ms");
                }
                Ok(Self::Swipe {
                    from: (x0, y0),
                    to: (x1, y1),
                    ms,
                })
            }
            _ => Err("display-test status|page|pattern|swipe"),
        }
    }
}

#[cfg(target_os = "none")]
mod device {
    use super::Command;
    use crate::{
        services::display::lvgl_port::DisplayPort,
        shell::{
            app::{AppMsg, MosaicoApp},
            navigation::route::{AppId, Page},
        },
    };
    use alloc::{format, string::String};
    use core::cell::Cell;
    use critical_section::Mutex;
    use embassy_futures::select::{select, Either};
    use embassy_time::{Duration, Instant, Timer};
    use esp_hal::gpio::Input;
    use rs_lvgl::sys as s;
    use rs_lvgl_ui::Ui;

    static REQUEST: Mutex<Cell<Option<Command>>> = Mutex::new(Cell::new(None));
    // count, min, mean, max, timeouts. GPIO edge observation includes scheduling.
    static TE: Mutex<Cell<[u64; 5]>> = Mutex::new(Cell::new([0; 5]));
    static TONE_REQUEST: embassy_sync::signal::Signal<
        embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
        (),
    > = embassy_sync::signal::Signal::new();
    static TONE_BUSY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
    const TONE_PATH: &str = "/data/.display-test-20261112.wav";

    #[embassy_executor::task]
    pub async fn tone_task() {
        use core::sync::atomic::Ordering;
        loop {
            TONE_REQUEST.wait().await;
            let result = crate::services::audio::task::execute(
                crate::services::audio::Command::Play(TONE_PATH.into()),
            )
            .await;
            log::info!("display.tone result={}", result);
            TONE_BUSY.store(false, Ordering::Release);
        }
    }

    pub async fn sample_te(te: &mut Input<'static>) {
        let mut last = None;
        let mut summary = [0u64, u64::MAX, 0, 0, 0];
        for _ in 0..17 {
            match select(te.wait_for_rising_edge(), Timer::after_millis(40)).await {
                Either::First(()) => {
                    let now = crate::platform::runtime::time::now_us();
                    if let Some(last) = last {
                        let period = now - last;
                        summary[0] += 1;
                        summary[1] = summary[1].min(period);
                        summary[2] += period;
                        summary[3] = summary[3].max(period);
                    }
                    last = Some(now);
                }
                Either::Second(()) => {
                    summary[4] += 1;
                    last = None;
                }
            }
        }
        if summary[0] != 0 {
            summary[2] /= summary[0];
        } else {
            summary[1] = 0;
        }
        critical_section::with(|cs| TE.borrow(cs).set(summary));
        log::info!(
            "display.te count={} min={} mean={} max={} timeouts={}",
            summary[0],
            summary[1],
            summary[2],
            summary[3],
            summary[4]
        );
    }

    pub async fn execute(command: Command) -> String {
        match command {
            Command::Tone(action) => {
                use crate::services::audio::{self, Command as Audio};
                if action == 1 {
                    use core::sync::atomic::Ordering;
                    if TONE_BUSY.swap(true, Ordering::AcqRel) {
                        return "@err display-test tone busy".into();
                    }
                    TONE_REQUEST.signal(());
                    return "@ok display-test tone queued".into();
                }
                if action == 2 {
                    let _ = audio::task::execute(Audio::Stop).await;
                }
                let result = crate::platform::runtime::executors::services::run(move || {
                    fs::global::with(|vfs| {
                        if action == 2 {
                            return vfs.unlink(TONE_PATH);
                        }
                        if vfs.stat(TONE_PATH).is_ok() {
                            return Err(fs::Errno::EXIST);
                        }
                        let frames = 16_000usize * 30;
                        let mut wave = alloc::vec::Vec::with_capacity(44 + frames * 2);
                        wave.extend_from_slice(
                            &audio::wav::header(audio::Format::SPEECH, (frames * 2) as u32)
                                .unwrap(),
                        );
                        // 500 Hz triangle, peak -18 dBFS, deterministic in both builds.
                        for index in 0..frames {
                            let phase = (index % 32) as i16;
                            let sample = if phase < 16 {
                                phase * 512 - 4096
                            } else {
                                12288 - phase * 512
                            };
                            wave.extend_from_slice(&sample.to_le_bytes());
                        }
                        vfs.write_file(TONE_PATH, &wave)
                    })
                })
                .await;
                return match result {
                    Ok(()) => "@ok display-test tone ready".into(),
                    Err(error) => format!("@err display-test tone {error}"),
                };
            }
            Command::Status => {
                let t = critical_section::with(|cs| TE.borrow(cs).get());
                format!("@ok display-test rgb888={} low_latency={} te_count={} te_min={} te_mean={} te_max={} timeouts={}",
                    cfg!(feature="display-rgb888") as u8, cfg!(feature="display-low-latency") as u8, t[0],t[1],t[2],t[3],t[4])
            }
            Command::Swipe { from, to, ms } => {
                use crate::services::input::touch::{inject_test, TouchEvent};
                let start = Instant::now();
                log::info!(
                    "display.synthetic begin={} duration_ms={}",
                    crate::platform::runtime::time::now_us(),
                    ms
                );
                let steps = u32::from(ms).div_ceil(10);
                for step in 0..=steps {
                    Timer::at(
                        start
                            + Duration::from_millis(
                                u64::from(step) * u64::from(ms) / u64::from(steps),
                            ),
                    )
                    .await;
                    let interpolate = |a: u16, b: u16| {
                        (i32::from(a) + (i32::from(b) - i32::from(a)) * step as i32 / steps as i32)
                            as u16
                    };
                    inject_test(TouchEvent {
                        at_us: crate::platform::runtime::time::now_us(),
                        x: interpolate(from.0, to.0),
                        y: interpolate(from.1, to.1),
                        pressed: true,
                    });
                }
                Timer::after_millis(10).await;
                inject_test(TouchEvent {
                    at_us: crate::platform::runtime::time::now_us(),
                    x: to.0,
                    y: to.1,
                    pressed: false,
                });
                "@ok display-test synthetic swipe complete".into()
            }
            _ => {
                let accepted = critical_section::with(|cs| {
                    let request = REQUEST.borrow(cs);
                    if request.get().is_some() {
                        false
                    } else {
                        request.set(Some(command));
                        true
                    }
                });
                if accepted {
                    "@ok display-test queued".into()
                } else {
                    "@err display-test busy".into()
                }
            }
        }
    }

    /// Owned and polled only by the existing LVGL/UI task, between uploads.
    pub struct UiHarness {
        overlay: *mut s::lv_obj_t,
    }
    impl UiHarness {
        pub fn new() -> Self {
            Self {
                overlay: core::ptr::null_mut(),
            }
        }
        pub fn poll(
            &mut self,
            app: &mut MosaicoApp,
            ui: &mut Ui<AppMsg>,
            port: &mut DisplayPort,
            sliding: bool,
        ) {
            if port.is_busy() || sliding {
                return;
            }
            let Some(request) = critical_section::with(|cs| REQUEST.borrow(cs).take()) else {
                return;
            };
            if !self.overlay.is_null() {
                unsafe {
                    s::lv_obj_delete(self.overlay);
                }
                self.overlay = core::ptr::null_mut();
            }
            match request {
                Command::Page(page) => app.go(match page {
                    0 => Page::Home,
                    1 => Page::Launcher,
                    _ => Page::App(AppId::Settings),
                }),
                Command::Pattern(pattern) if pattern != 0 => {
                    app.go(Page::Launcher);
                    unsafe {
                        self.overlay = pattern_object(pattern);
                    }
                }
                _ => {}
            }
            port.invalidate_all();
            ui.mark_dirty();
        }
    }

    unsafe fn rect(
        parent: *mut s::lv_obj_t,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        color: u32,
    ) -> *mut s::lv_obj_t {
        unsafe {
            let object = s::lv_obj_create(parent);
            s::lv_obj_remove_style_all(object);
            s::lv_obj_set_pos(object, x, y);
            s::lv_obj_set_size(object, w, h);
            s::lv_obj_remove_flag(
                object,
                s::lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE | s::lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE,
            );
            s::lv_obj_set_style_bg_color(object, s::lv_color_hex(color), 0);
            s::lv_obj_set_style_bg_opa(object, 255, 0);
            object
        }
    }
    unsafe fn pattern_object(pattern: u8) -> *mut s::lv_obj_t {
        unsafe {
            let root = rect(s::lv_layer_top(), 0, 0, 480, 480, 0);
            let colors = [0xff0000, 0x00ff00, 0x0000ff, 0xffffff, 0x808080, 0x000000];
            if pattern == 1 {
                for (index, color) in colors.into_iter().enumerate() {
                    rect(root, index as i32 * 80, 0, 80, 480, color);
                }
            } else {
                for (index, color) in [0xffffff, 0xff0000, 0x00ff00, 0x0000ff]
                    .into_iter()
                    .enumerate()
                {
                    let bar = rect(root, 0, index as i32 * 108, 480, 108, 0);
                    s::lv_obj_set_style_bg_grad_color(bar, s::lv_color_hex(color), 0);
                    s::lv_obj_set_style_bg_grad_dir(bar, s::lv_grad_dir_t_LV_GRAD_DIR_HOR, 0);
                }
                for (index, color) in colors.into_iter().enumerate() {
                    rect(root, index as i32 * 80, 432, 80, 48, color);
                }
                let circle = rect(root, 190, 165, 100, 100, 0xffffff);
                s::lv_obj_set_style_radius(circle, 50, 0);
                s::lv_obj_set_style_bg_opa(circle, 128, 0);
            }
            root
        }
    }
}
#[cfg(target_os = "none")]
pub use device::*;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trajectories_reject_invalid_coordinates_and_duration_without_truncation() {
        assert!(Command::parse(&["swipe", "0", "479", "479", "0", "2000"]).is_ok());
        for args in [
            ["swipe", "480", "0", "1", "1", "500"],
            ["swipe", "-1", "0", "1", "1", "500"],
            ["swipe", "65536", "0", "1", "1", "500"],
            ["swipe", "1", "0", "1", "1", "19"],
            ["swipe", "1", "0", "1", "1", "2001"],
        ] {
            assert!(Command::parse(&args).is_err());
        }
    }
}
