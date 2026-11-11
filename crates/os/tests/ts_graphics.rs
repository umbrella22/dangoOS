#![cfg(feature = "ts-engine")]
use app_host::{DisplayInfo, HardwareRequest, HardwareSnapshot};
use os::apps::{
    ts::graphics::Graphics,
    ts::host::{AppHost, Platform},
};
use std::{cell::RefCell, path::Path, rc::Rc};
use ts_engine::{workers::Group, CallState, HostReply, Limits, Package, Vm, GLOBAL_ALLOCATOR};

#[derive(Default)]
struct State {
    logs: Vec<String>,
    outputs: Vec<HardwareRequest>,
    phase: u32,
}
struct Board(Rc<RefCell<State>>);
impl Platform for Board {
    fn print(&mut self, text: &str, _: bool) {
        self.0.borrow_mut().logs.push(text.into());
    }
    fn now_ms(&self) -> u64 {
        1000 + self.0.borrow().phase as u64 * 200
    }
    fn hardware(&mut self) -> HardwareSnapshot {
        let phase = self.0.borrow().phase;
        let mut s = HardwareSnapshot {
            available: 63,
            valid: 127,
            accel: [0, 0, 1000],
            soc: 50,
            led_generation: if phase == 4 { 2 } else { 1 },
            ..Default::default()
        };
        s.onboard.ready = 1;
        if phase >= 1 {
            s.onboard.edge(0, true, 1200);
        }
        if phase >= 2 {
            s.onboard.edge(0, false, 1400);
        }
        if phase == 5 {
            s.onboard.sequence = 25;
        }
        s
    }
    fn hardware_request(&mut self, r: HardwareRequest) -> Result<(), i32> {
        self.0.borrow_mut().outputs.push(r);
        Ok(())
    }
    fn display_info(&mut self) -> Result<DisplayInfo, i32> {
        Ok(DisplayInfo {
            width: 16,
            height: 16,
            touch_available: true,
        })
    }
}
fn run(file: &str) -> (Rc<RefCell<State>>, Rc<RefCell<Graphics>>, Vec<Vec<u8>>) {
    let state = Rc::new(RefCell::new(State::default()));
    let host = AppHost::new(Board(state.clone()), "{}", "/data/appdata/test".into()).unwrap();
    let graphics = host.graphics.clone();
    let mut vm = Vm::new(Box::new(host), Limits::default(), &GLOBAL_ALLOCATOR).unwrap();
    let bytes = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../resources/embedts/fixtures")
            .join(file),
    )
    .unwrap();
    unsafe { vm.load(Package::parse(&bytes).unwrap()) }.unwrap();
    assert_eq!(vm.start("main", None).unwrap(), CallState::Pending);
    let mut group = Group::new(Limits::default(), &GLOBAL_ALLOCATOR);
    let mut frames = Vec::new();
    let mut done = false;
    for now in 0..10000 {
        vm.drain_jobs(32).unwrap();
        while let Some(queued) = vm.take_request() {
            let r = match queued {
                ts_engine::Request::Delay { id, .. } => {
                    state.borrow_mut().phase += 1;
                    vm.complete_void(id).unwrap();
                    continue;
                }
                ts_engine::Request::Host(request) => request,
            };
            let id = r.id;
            if r.operation.starts_with("thread.") {
                group.command(r, now);
                continue;
            }
            let reply = match r.operation.as_str() {
                "display.present" => {
                    let frame = graphics.borrow_mut().present(&r.args).unwrap();
                    let changed = !frame.rectangles.is_empty();
                    if changed {
                        assert_eq!(frame.width, 16);
                        assert_eq!(frame.pixels.len(), 512);
                        frames.push((*frame.pixels).clone());
                    }
                    graphics.borrow_mut().presented();
                    HostReply::json(if changed { "true" } else { "false" })
                }
                "sleep" => {
                    state.borrow_mut().phase += 1;
                    HostReply::json("null")
                }
                op => panic!("unexpected {op}"),
            };
            vm.complete(id, Ok(reply)).unwrap();
        }
        group.poll(now);
        while let Some((id, r)) = group.take_completion() {
            vm.complete(id, Ok(r)).unwrap();
        }
        if vm.entry_status().unwrap() == CallState::Done(42) {
            done = true;
            break;
        }
    }
    assert!(done);
    drop(group);
    assert_eq!(vm.close().used, 0);
    (state, graphics, frames)
}
#[test]
fn sdk_raster_images_and_native_handles_release() {
    let (state, graphics, frames) = run("ts-graphics.etb");
    assert_eq!(frames.len(), 2);
    assert_ne!(frames[0], frames[1]);
    assert!(graphics.borrow().screen.is_none());
    let stats = graphics
        .borrow_mut()
        .call("display.stats", "[]", &[])
        .unwrap();
    assert!(stats.json.contains("\"native_bytes\":0"));
    assert!(state
        .borrow()
        .logs
        .iter()
        .any(|s| s == "graphics-parity:ok"));
    assert_eq!(state.borrow().outputs.len(), 3);
}
#[test]
fn sdk_button_errors_calibration_and_leases() {
    let (state, _, _) = run("ts-hardware.etb");
    assert!(state.borrow().logs.iter().any(|s| s == "hardware-edge:ok"));
}

#[test]
fn scalar_and_json_shapes_match_pixels_clipping_and_frame_state_errors() {
    let mut scalar = Graphics::new(1024 * 1024);
    let mut json = Graphics::new(1024 * 1024);
    let draw = ts_engine::Draw {
        operation: 0,
        coordinates: [0, 0, 8, 8, 0, 0],
        color: 0xff12ab34,
        start: 0.125,
        end: 2.75,
    };
    assert!(scalar.draw(draw).is_err());
    for g in [&mut scalar, &mut json] {
        g.call("display.open", "[32,32,2]", &[]).unwrap();
    }
    assert_eq!(
        scalar.draw(draw).unwrap_err().code,
        json.call(
            "display.draw",
            "[0,[0,0,8,8,0,0],4279413556,0.125,2.75]",
            &[]
        )
        .unwrap_err()
        .code
    );
    for op in 0..9 {
        for g in [&mut scalar, &mut json] {
            g.call("display.begin", "[4278190080]", &[]).unwrap();
            g.call("display.save", "[]", &[]).unwrap();
            g.call("display.translate", "[-2,3]", &[]).unwrap();
            g.call("display.clip", "[1,1,29,28]", &[]).unwrap();
        }
        let draw = ts_engine::Draw {
            operation: op,
            coordinates: [3, 4, 12, 11, 4, 16],
            ..draw
        };
        scalar.draw(draw).unwrap();
        json.call(
            "display.draw",
            &serde_json::json!([op, draw.coordinates, draw.color, draw.start, draw.end])
                .to_string(),
            &[],
        )
        .unwrap();
        for g in [&mut scalar, &mut json] {
            g.call("display.restore", "[]", &[]).unwrap();
        }
        let a = scalar.present("[false]").unwrap();
        let b = json.present("[false]").unwrap();
        assert_eq!(a.pixels, b.pixels, "shape {op}");
        assert_eq!(a.rectangles.len(), b.rectangles.len());
        scalar.presented();
        json.presented();
    }
}

#[test]
fn inline_damage_and_borrowed_blits_retain_pixels_after_the_display_closes() {
    let mut graphics = Graphics::new(1024 * 1024);
    graphics.call("display.open", "[128,128,1]", &[]).unwrap();
    graphics.call("display.begin", "[4278190080]", &[]).unwrap();
    drop(graphics.present("[false]").unwrap());
    graphics.presented();
    graphics.call("display.begin", "[null]", &[]).unwrap();
    for (x, y) in [(0, 0), (120, 0), (0, 120), (120, 120)] {
        graphics
            .draw(ts_engine::Draw {
                operation: 0,
                coordinates: [x, y, 3, 3, 0, 0],
                color: 0xffff0000,
                start: 0.,
                end: 0.,
            })
            .unwrap();
    }
    let frame = graphics.present("[false]").unwrap();
    assert_eq!(frame.rectangles.len(), 4);
    graphics.presented();
    graphics.call("display.close", "[]", &[]).unwrap();
    assert_eq!((&frame).into_iter().count(), 4);
    for (blit, rect) in (&frame).into_iter().zip(&frame.rectangles) {
        assert_eq!(blit.stride, 256);
        assert_eq!((blit.x0, blit.y0), (rect.x as u16, rect.y as u16));
        assert_eq!((blit.x1 - blit.x0, blit.y1 - blit.y0), (3, 3));
        let offset = usize::from(blit.y0) * blit.stride + usize::from(blit.x0) * 2;
        assert_eq!(blit.pixels, frame.pixels[offset..].as_ptr());
        assert_eq!(&frame.pixels[offset..offset + 2], &[0, 248]);
    }
}

#[test]
fn publications_reuse_storage_copy_damage_and_preserve_retained_frames() {
    let mut g = Graphics::new(1024 * 1024);
    g.call("display.open", "[16,16,1]", &[]).unwrap();
    g.call("display.begin", "[4278190080]", &[]).unwrap();
    let old = g.present("[false]").unwrap();
    let old_bytes = old.pixels.to_vec();
    g.presented();
    g.call("display.begin", "[null]", &[]).unwrap();
    g.call("display.draw", "[0,[0,0,2,2,0,0],4294901760,0,0]", &[])
        .unwrap();
    let changed = g.present("[false]").unwrap();
    assert_ne!(&changed.pixels[..2], &old.pixels[..2]);
    assert_eq!(*old.pixels, old_bytes);
    drop(old);
    let address = changed.pixels.as_ptr();
    drop(changed);
    g.presented();
    for n in 0..128 {
        g.call("display.begin", "[null]", &[]).unwrap();
        g.draw(ts_engine::Draw {
            operation: 0,
            coordinates: [n % 12, (n * 7) % 12, 3, 3, 0, 0],
            color: 0xff000000 | (n as u32 * 12345),
            start: 0.0,
            end: 0.0,
        })
        .unwrap();
        let published = g.present("[false]").unwrap();
        assert_eq!(
            published.pixels.as_slice(),
            g.screen.as_ref().unwrap().pixels()
        );
        assert_eq!(published.pixels.as_ptr(), address);
        drop(published);
        g.presented();
        g.call("display.begin", "[null]", &[]).unwrap();
        let clean = g.present("[false]").unwrap();
        assert!(clean.rectangles.is_empty());
        assert_eq!((&clean).into_iter().count(), 0);
        assert_eq!(clean.pixels.as_ptr(), address);
        drop(clean);
        g.presented();
    }
    let stats: serde_json::Value =
        serde_json::from_str(&g.call("display.stats", "[]", &[]).unwrap().json).unwrap();
    assert_eq!(stats["publication_bytes"], 512);
    assert_eq!(stats["publication_allocations"], 2);
    assert!(stats["publication_copied_bytes"].as_u64().unwrap() < 512 * 130);
    g.call("display.close", "[]", &[]).unwrap();
    assert!(g
        .call("display.stats", "[]", &[])
        .unwrap()
        .json
        .contains("\"native_bytes\":0"));

    // A retained snapshot must obey the graphics budget; releasing it allows
    // the already allocated buffer to recover without losing frame state.
    let mut bounded = Graphics::new(1024);
    bounded.call("display.open", "[16,16,1]", &[]).unwrap();
    bounded.call("display.begin", "[4278190080]", &[]).unwrap();
    let held = bounded.present("[false]").unwrap();
    bounded.presented();
    bounded.call("display.begin", "[null]", &[]).unwrap();
    bounded
        .call("display.draw", "[0,[0,0,2,2,0,0],4294901760,0,0]", &[])
        .unwrap();
    assert_eq!(
        bounded.present("[false]").err().unwrap().code,
        app_host::esp_err::NO_MEM
    );
    drop(held);
    let recovered = bounded.present("[false]").unwrap();
    assert_eq!(&recovered.pixels[..2], &[0, 248]);
}
#[test]
fn raster_pixels_double_buffer_receipts_and_image_errors() {
    let mut g = Graphics::new(1024 * 1024);
    g.call("display.open", "[4,4,2]", &[]).unwrap();
    g.call("display.begin", "[4278190080]", &[]).unwrap();
    g.call("display.draw", "[0,[0,0,2,2,0,0],4294901760,0,0]", &[])
        .unwrap();
    let f = g.present("[false]").unwrap();
    assert_eq!(&f.pixels[..4], &[0, 248, 0, 248]);
    assert_eq!(&f.pixels[4..8], &[0, 0, 0, 0]);
    g.presented();
    g.call("display.begin", "[null]", &[]).unwrap();
    assert!(g.present("[false]").unwrap().rectangles.is_empty());
    assert_eq!(&f.pixels[..2], &[0, 248]);
    g.presented();
    assert!(g.call("font.decode", "[]", b"bad-font").is_err());
    assert!(g.call("image.decode", "[]", b"not-an-image").is_err());
    assert!(g.call("image.raw", "[1,1,\"RGB3\"]", &[1, 2]).is_err());
    assert!(g.call("image.info", "[0]", &[]).is_err());
    let font = g
        .call("font.decode", "[]", include_bytes!("fixtures/one.dfn"))
        .unwrap()
        .json;
    let measurement = g
        .call("display.measure", &format!("[\"A\",8,{font}]"), &[])
        .unwrap();
    assert!(measurement.json.contains("\"height\":8"));
    g.call("font.close", &format!("[{font}]"), &[]).unwrap();
    assert!(g
        .call("display.measure", &format!("[\"A\",8,{font}]"), &[])
        .is_err());
    let png = include_bytes!("fixtures/alpha.png");
    let id = g.call("image.decode", "[]", png).unwrap().json;
    let out = g.call("image.bytes", &format!("[{id}]"), &[]).unwrap();
    assert_eq!(out.bytes, [128, 0, 0, 0, 255, 0]);
    g.call("image.close", &format!("[{id}]"), &[]).unwrap();
    assert!(g.call("image.bytes", &format!("[{id}]"), &[]).is_err());
    let mut small = Graphics::new(10);
    assert!(small.call("display.open", "[4,4,1]", &[]).is_err());
}
