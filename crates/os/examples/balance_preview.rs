//! Export the production runtime/animator/renderer under scripted IMU input.
//! cargo run -p os --example balance_preview -- OUTPUT [SKIN_ID] [SKIN_ROOT]
use os::companion::{
    behavior::affect::{EventKind, EventSource, SensorEvent},
    render::{FoxAnimator, FoxInput, FoxPose, FoxRenderer, H, W, skin::Skin},
};
use os::{Expression, FoxControl, Runtime};
use std::{
    fs,
    io::{BufWriter, Write},
    path::Path,
    rc::Rc,
};

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let out = Path::new(args.get(1).expect("output directory"));
    let id = args.get(2).map_or("default", String::as_str);
    let root = args.get(3).map_or("resources/skins", String::as_str);
    let skin = Skin::parse(fs::read(format!("{root}/{id}/fox.bin")).unwrap()).unwrap();
    for (case, duration, turn_ms, rate) in [
        ("idle", 3400u64, 0u64, 0.0f32),
        ("slow", 4600u64, 3400u64, 50.0f32),
        ("rescue", 2600, 240, 360.0),
        ("fall", 3800, 800, 360.0),
        ("heavy", 4000, 900, 600.0),
    ] {
        let dir = out.join(id).join(case);
        fs::create_dir_all(&dir).unwrap();
        let control = FoxControl::new(Expression::Idle);
        let mut runtime = Runtime::new(0, Rc::clone(&control));
        let mut animator = FoxAnimator::with_skin(FoxPose::Idle, Rc::clone(&skin));
        let mut renderer = FoxRenderer::new();
        let mut pixels = vec![0; W * H];
        let mut trace = Vec::new();
        let mut frame = 0;
        let mut render_us = Vec::new();
        for at in (10..=duration).step_by(10) {
            let moving = at.saturating_sub(500).min(turn_ms);
            let angle = moving as f32 * rate / 1000.0;
            if at % 20 == 0 {
                let a = angle.to_radians();
                for kind in [
                    EventKind::Accel {
                        ax_mg: (a.sin() * 1000.0) as i16,
                        ay_mg: (a.cos() * 1000.0) as i16,
                        az_mg: 0,
                    },
                    EventKind::Gyro {
                        gx_dps: 0,
                        gy_dps: 0,
                        gz_dps: if at > 500 && moving < turn_ms {
                            rate as i16
                        } else {
                            0
                        },
                    },
                ] {
                    runtime.on_sensor(SensorEvent {
                        at_ms: at,
                        source: EventSource::Imu,
                        kind,
                    });
                }
            }
            runtime.tick(at);
            animator.set_pose(control.expression().into());
            animator.advance(10);
            if at % 40 != 0 {
                continue;
            }
            let balance = control.balance();
            let scene = animator.scene(FoxInput {
                balance,
                ..FoxInput::REST
            });
            let start = std::time::Instant::now();
            renderer.render(&scene, &mut pixels);
            render_us.push(start.elapsed().as_micros());
            let mut file =
                BufWriter::new(fs::File::create(dir.join(format!("{frame:04}.ppm"))).unwrap());
            write!(file, "P6\n{W} {H}\n255\n").unwrap();
            for c in &pixels {
                file.write_all(&[
                    ((c >> 11) * 255 / 31) as u8,
                    (((c >> 5) & 63) * 255 / 63) as u8,
                    ((c & 31) * 255 / 31) as u8,
                ])
                .unwrap();
            }
            trace.push(serde_json::json!({"frame":frame,"at_ms":at,"device_angle":angle,"stage":format!("{:?}",balance.stage),"face":format!("{:?}",balance.face),"rotation_reaction":balance.rotation_reaction,"angle":balance.angle_deg,"x":balance.x,"y":balance.y,"reach":balance.reach,"grip":balance.grip,"drag":balance.drag,"rig":scene.rig.is_some(),"expression":format!("{:?}",control.expression())}));
            frame += 1;
        }
        render_us.sort_unstable();
        fs::write(
            dir.join("trace.json"),
            serde_json::to_vec_pretty(&trace).unwrap(),
        )
        .unwrap();
        println!(
            "{id}/{case}: {frame} frames; host render p95={} us, max={} us",
            render_us[render_us.len() * 95 / 100],
            render_us.last().unwrap()
        );
    }
}
