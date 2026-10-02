//! Slint 平台层(Mosaic UI): 软件渲染 → QSPI GRAM 刷新, 触摸 → 官方 Hub UI
//!
//! Hub、抽屉、锁屏与应用页。时间/电量/天气数据仍为占位，
//! 后续里程碑接入 WiFi(天气/真实时间)、BQ27220(电量)、BMI270/TOF/相机。
//! ReusedBuffer 脏区渲染后同步写整帧；发布失败保留帧缓冲并重试。

use alloc::{boxed::Box, rc::Rc, vec::Vec};
use core::{cell::Cell, time::Duration};
use esp_hal::{
    delay::Delay,
    gpio::{Input, Level},
    time::Instant,
};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use slint::{
    platform::{
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        PointerEventButton, WindowEvent,
    },
    ComponentHandle, LogicalPosition, ModelRc, VecModel,
};

use crate::drivers::{
    breakout::Game, camera::CameraModule, display::Display, fan::FanPwm, imu::Bmi270,
    touch::Cst9220,
};
use crate::runtime::{
    app_active, Brightness, GameContact, Periodic, Publication, GAME_STEP_US, IMU_SAMPLE_US,
};
use log::info;

fn now_us() -> u64 {
    Instant::now().duration_since_epoch().as_micros()
}

struct MosaicPlatform {
    window: Rc<MinimalSoftwareWindow>,
}

impl Platform for MosaicPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        Duration::from_micros(Instant::now().duration_since_epoch().as_micros())
    }
}

/// 开机基准时刻(10:08)，RTC/NTP 接入前用 uptime 推算占位时钟。
const CLOCK_BASE_MINUTES: u64 = 10 * 60 + 8;

pub fn run(
    display: &mut Display,
    touch: &mut Option<Cst9220>,
    fan: &mut Option<FanPwm>,
    ai_key: &mut Input<'static>,
    mut imu: Option<Bmi270>,
    camera: &mut CameraModule,
) -> ! {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    window.set_size(slint::PhysicalSize::new(
        crate::board::LCD_WIDTH.into(),
        crate::board::LCD_HEIGHT.into(),
    ));
    slint::platform::set_platform(Box::new(MosaicPlatform {
        window: window.clone(),
    }))
    .expect("Slint 平台只能初始化一次");

    let ui = fan_ui::MosaicApp::new().expect("创建 Mosaic UI 失败");
    ui.show().expect("显示窗口失败");
    let win = ui.window();

    // Slint owns navigation; callbacks only enqueue domain actions.
    let game_start = Rc::new(Cell::new(false));
    ui.on_game_start({
        let request = game_start.clone();
        move || request.set(true)
    });

    let camera_retry = Rc::new(Cell::new(false));
    ui.on_camera_retry_requested({
        let request = camera_retry.clone();
        move || request.set(true)
    });

    crate::quick_feedback::install(&ui, Duration::from_millis(2500));
    let mut screen_contact = crate::runtime::screen_contact::ScreenContact::default();
    let mut touching = false;
    let mut game_contact = GameContact::default();
    let mut last_pos = LogicalPosition::new(0.0, 0.0);
    let mut ai_seen = false;
    let mut imu_poll = Periodic::new(IMU_SAMPLE_US);
    let mut game = Game::new();
    let mut game_tick = Periodic::new(GAME_STEP_US);
    let bricks_model = Rc::new(VecModel::from(
        (0..crate::drivers::breakout::BRICK_COUNT)
            .map(|i| game.bricks & (1 << i) != 0)
            .collect::<Vec<_>>(),
    ));
    ui.set_game_bricks_live(ModelRc::from(bricks_model.clone()));
    let delay = Delay::new();
    let boot_us = now_us();
    let mut publication = Publication::default();
    let mut brightness = Brightness::default();

    // 心跳(每 5s): 渲染耗时, 供台架调优
    let mut hb_last = now_us();
    let mut hb_renders = 0usize;
    let mut hb_render_us = 0usize;
    let mut hb_render_max = 0usize;

    loop {
        game_contact.update(app_active(
            ui.get_app(),
            3,
            ui.get_locked(),
            ui.get_drawer_open(),
        ));
        // ---- 触摸 → Slint 输入 ----
        if let Some(tp) = touch {
            match tp.poll() {
                Ok(state) => match state {
                    crate::drivers::touch::TouchState::Down { x, y } => {
                        if screen_contact.sample(x as f32, y as f32, ui.get_screen_corner_radius())
                        {
                            if !touching {
                                game_contact.begin(
                                    app_active(
                                        ui.get_app(),
                                        3,
                                        ui.get_locked(),
                                        ui.get_drawer_open(),
                                    ),
                                    y as f32,
                                );
                            }
                            let pos = LogicalPosition::new(x as f32, y as f32);
                            win.dispatch_event(WindowEvent::PointerMoved { position: pos });
                            if !touching {
                                win.dispatch_event(WindowEvent::PointerPressed {
                                    position: pos,
                                    button: PointerEventButton::Left,
                                });
                                touching = true;
                                info!("触摸按下: ({x},{y})");
                            }
                            last_pos = pos;
                        }
                    }
                    crate::drivers::touch::TouchState::Up => {
                        screen_contact.release();
                        if touching {
                            win.dispatch_event(WindowEvent::PointerReleased {
                                position: last_pos,
                                button: PointerEventButton::Left,
                            });
                            touching = false;
                            game_contact.end();
                        }
                    }
                },
                Err(_) => screen_contact.transport_error(),
            }
        }

        slint::platform::update_timers_and_animations();

        // ---- 状态栏时钟: uptime 推算(无 RTC 占位; 表盘指针 M1 为静态 10:10) ----
        let elapsed_min = (now_us().saturating_sub(boot_us) / 60_000_000) % (24 * 60);
        let total_min = (CLOCK_BASE_MINUTES + elapsed_min) % (24 * 60);
        let hours = (total_min / 60) as i32;
        let minutes = (total_min % 60) as i32;
        ui.set_status_time(alloc::format!("{hours:02}:{minutes:02}").into());

        // GPIO12 remains unclaimed by default. No synthetic RPM is presented.
        if let Some(fan) = fan.as_mut() {
            fan.tick();
        }
        ui.set_fan_duty(fan.as_ref().map_or(0, |fan| fan.duty()) as i32);
        ui.set_fan_rpm(0);
        ui.set_fan_available(fan.is_some());

        if let Some(value) = brightness.requested(ui.get_brightness()) {
            brightness.completed(value, display.set_brightness(value).is_ok());
        }

        // AI 键(GPIO7, 低有效): 切换锁屏
        if ai_key.level() == Level::Low {
            if !ai_seen {
                ai_seen = true;
                let locked = ui.get_locked();
                ui.set_locked(!locked);
                info!("AI 键: {}", if locked { "解锁" } else { "锁屏" });
            }
        } else {
            ai_seen = false;
        }

        let camera_active = app_active(ui.get_app(), 6, ui.get_locked(), ui.get_drawer_open());
        if let Some(frame) = camera.poll(camera_active, camera_retry.replace(false), now_us()) {
            ui.set_camera_frame(slint::Image::from_rgb8(frame));
        }
        ui.set_camera_preview_ready(camera.preview_ready());
        ui.set_camera_status(camera.status().into());
        ui.set_camera_retry_available(camera.retry_available());

        let imu_active = app_active(ui.get_app(), 1, ui.get_locked(), ui.get_drawer_open());
        if imu_poll.due(now_us(), imu_active) > 0 {
            let sample = imu.as_mut().and_then(|imu| imu.read_accel_mg());
            ui.set_imu_valid(sample.is_some());
            if let Some((x, y, z)) = sample {
                let pitch = libm::atan2f(x, libm::sqrtf(z * z + y * y)) * 57.2958;
                let roll = libm::atan2f(y, libm::sqrtf(z * z + x * x)) * 57.2958;
                let tilt = libm::sqrtf(pitch * pitch + roll * roll);
                // Coordinates are the bubble's top-left in the full 480px window.
                ui.set_bubble_x(202.0 + libm::sinf(roll * 0.0174533) * 120.0);
                ui.set_bubble_y(166.5 + libm::sinf(pitch * 0.0174533) * 75.0);
                ui.set_imu_angle(alloc::format!("{:.0}°", tilt.min(90.0)).into());
                ui.set_imu_pitch(alloc::format!("{pitch:.0}").into());
                ui.set_imu_roll(alloc::format!("{roll:.0}").into());
            }
        }

        let game_active = app_active(ui.get_app(), 3, ui.get_locked(), ui.get_drawer_open());
        if game_start.replace(false) && game_active {
            game.start();
        }
        game_contact.update(game_active);
        if game_active && touching && game_contact.owned() {
            game.set_paddle_target(last_pos.x);
        }
        for _ in 0..game_tick.due(now_us(), game_active) {
            game.step();
        }
        // Publish immediate start/reset state as well as fixed physics updates.
        ui.set_game_score(game.score as i32);
        ui.set_game_lives(game.lives as i32);
        ui.set_game_running(game.running);
        ui.set_game_ball_x(game.ball_x);
        ui.set_game_ball_y(game.ball_y);
        ui.set_game_paddle_x(game.paddle_x);
        use slint::Model;
        for i in 0..crate::drivers::breakout::BRICK_COUNT {
            let live = game.bricks & (1 << i) != 0;
            if bricks_model.row_data(i) != Some(live) {
                bricks_model.set_row_data(i, live);
            }
        }

        if !publication.pending() {
            let r0 = now_us();
            let mut rendered = false;
            window.draw_if_needed(|renderer| {
                renderer.render(display.fb_mut(), crate::board::LCD_WIDTH as usize);
                rendered = true;
            });
            if rendered {
                let dur = now_us().wrapping_sub(r0) as usize;
                hb_renders += 1;
                hb_render_us = hb_render_us.wrapping_add(dur);
                hb_render_max = hb_render_max.max(dur);
                publication.rendered();
            }
        }

        if publication.pending() {
            publication.completed(display.present().is_ok());
        }

        if now_us().saturating_sub(hb_last) >= 5_000_000 {
            let ts = touch.as_ref().map(|tp| tp.stats());
            info!(
                "心跳 渲染={hb_renders} avg={}us max={}us DMA错误={} 触摸轮询={}",
                if hb_renders > 0 {
                    hb_render_us / hb_renders
                } else {
                    0
                },
                hb_render_max,
                crate::drivers::display::stats().dma_err,
                ts.map(|t| t.polls).unwrap_or(0),
            );
            let cs = camera.stats();
            info!(
                "相机 帧={} 错误={} 超时={} 收到={}B",
                cs.frames, cs.errors, cs.timeouts, cs.received
            );
            hb_last = now_us();
            hb_renders = 0;
            hb_render_us = 0;
            hb_render_max = 0;
        }

        delay.delay_millis(8);
    }
}
