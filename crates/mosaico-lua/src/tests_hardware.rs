//! Contract tests exercise the real C/Lua bridge, not a parallel Rust model.
use crate::*;
use alloc::{boxed::Box, string::String, vec::Vec};
use core::cell::Cell;
std::thread_local! {static NOW: Cell<u64> = const {Cell::new(1_000_000)};}
fn clock() -> u64 {
    NOW.with(Cell::get)
}
fn advance(ms: u32) {
    NOW.with(|n| n.set(n.get() + ms as u64 * 1000));
}
#[derive(Default)]
struct Board {
    snapshot: HardwareSnapshot,
    requests: Vec<HardwareRequest>,
    reject: bool,
    lines: Vec<String>,
    shared: Option<std::sync::Arc<std::sync::Mutex<Vec<HardwareRequest>>>>,
}
impl Board {
    fn present() -> Self {
        let mut snapshot = HardwareSnapshot {
            available: 63,
            valid: 127,
            accel: [-32768, 0, 32767],
            gyro: [0, -1, 1],
            temperature: 512,
            sensor_time: 12345,
            imu_status: 0xc0,
            magnetic: [0.0, -20.0, 30.0],
            magnet_status: 0x42,
            magnet_state: 2,
            voltage_mv: 4200,
            current_ma: 120,
            soc: 100,
            charging: 1,
            ..HardwareSnapshot::default()
        };
        snapshot.button_board(Some([11, 10]));
        snapshot.valid |= 64;
        Self {
            snapshot,
            ..Self::default()
        }
    }
}
impl Host for Board {
    fn print(&mut self, line: &[u8]) {
        self.lines.push(String::from_utf8_lossy(line).into_owned());
    }
    fn hardware_snapshot(&mut self) -> HardwareSnapshot {
        self.snapshot
    }
    fn hardware_request(&mut self, request: HardwareRequest) -> Result<(), i32> {
        if self.reject {
            return Err(esp_err::NOT_SUPPORTED);
        }
        if let Some(shared) = &self.shared {
            shared.lock().unwrap().push(request);
        }
        self.requests.push(request);
        Ok(())
    }
}
fn vm(board: Board, source: &str) -> Vm {
    NOW.with(|n| n.set(1_000_000));
    let mut vm = Vm::new(
        Box::new(board),
        Limits {
            clock: Some(clock),
            ..Limits::default()
        },
        &GLOBAL_ALLOCATOR,
        7,
    )
    .unwrap();
    vm.load("hardware-test", source.as_bytes()).unwrap();
    vm
}
fn finish(vm: &mut Vm) {
    for _ in 0..10000 {
        match vm.resume() {
            Step::Done => return,
            Step::Slice => {}
            Step::Request(Request::Sleep { ms }) => advance(ms),
            other => panic!("unexpected Lua result {other:?}"),
        }
    }
    panic!("script did not finish")
}
fn sleep(vm: &mut Vm) {
    loop {
        match vm.resume() {
            Step::Slice => {}
            Step::Request(Request::Sleep { ms }) => {
                advance(ms);
                return;
            }
            other => panic!("expected sleep, got {other:?}"),
        }
    }
}

#[test]
fn official_board_sensor_examples_run_unchanged() {
    for source in [
        include_str!("../vendor/esp-claw/hardware-contracts/imu/test/imu_read.lua"),
        include_str!(
            "../vendor/esp-claw/hardware-contracts/magnetometer/test/magnetometer_read.lua"
        ),
    ] {
        let mut vm = vm(Board::present(), source);
        vm.set_args_json(r#"{"samples":2,"interval_ms":1}"#)
            .unwrap();
        finish(&mut vm);
        assert!(!vm.host_mut::<Board>().unwrap().lines.is_empty());
    }
}

#[test]
fn sensors_preserve_raw_units_zero_and_unavailable_states() {
    let source = r#"
      local imu=require('imu'); local s=imu.new({device='imu_sensor'})
      local a=s:read();assert(a.accel.x==-32768 and a.accel.y==0 and a.accel.z==32767)
      assert(a.gyro.y==-1 and a.sens_time==12345 and a.status==0xc0)
      assert(s:read_temperature()==512)
      assert(not pcall(s.read_int_status,s));assert(not pcall(imu.new,{device='imu_sensor'}))
      s:close();s:close();assert(not pcall(s.read,s))
      local m=require('magnetometer').new({device='magnetometer_sensor'})
      local b=m:read();assert(b.magnetic.x==0 and b.raw_magnetic.y==-20)
      assert(b.status==0x42 and b.proximity=='contact' and not b.calibrated and b.temperature==nil)
      assert(not pcall(m.read_temperature,m));assert(m:chip_id()==0x32);m:close()
      local g=require('lib_fuel_gauge').new({borrow=true,port=0,chip='bq27220'})
      local c=g:read();assert(c.soc==100 and c.voltage_mv==4200 and c.current_ma==120 and c.charging)
      assert(g:address()==0x55 and g:chip()=='bq27220');g:close()
      require('delay').delay_ms(1)
      s=imu.new({device='imu_sensor'});assert(not pcall(s.read,s));s:close()
      g=require('fuel_gauge').new({});assert(not pcall(g.read,g));g:close()
      assert(not pcall(require('magnetometer').new,{device='magnetometer_sensor'}))
    "#;
    let mut vm = vm(Board::present(), source);
    sleep(&mut vm);
    let board = vm.host_mut::<Board>().unwrap();
    board.snapshot.valid = 0;
    board.snapshot.available &= !2;
    finish(&mut vm);
}

#[test]
fn calibration_roundtrips_and_validates_before_mutation() {
    let mut vm = vm(
        Board::present(),
        r#"
      local m=require('magnetometer').new({device='magnetometer_sensor'})
      m:calibration_set(m:calibration_get())
      local c={hard_iron={1,2,3},soft_iron={{2,0,0},{0,3,0},{0,0,4}}}
      m:calibration_set(c);c.hard_iron[1]=1000
      local a=m:read();assert(a.magnetic.x==-2 and a.magnetic.y==-66 and a.magnetic.z==108)
      assert(not pcall(m.calibration_set,m,{hard_iron={1,2,3},soft_iron={{1,0,0},{0,1,0},{0,0,0/0}}}))
      assert(m:calibration_get().hard_iron[1]==1)
      m:calibration_reset();assert(not pcall(m.calibration_finish,m))
      for i=1,16 do m:calibration_add_sample({i,2*i,3*i}) end
      local b=m:calibration_finish();assert(b.sample_count==16 and b.calibrated and not b.collecting)
      assert(b.hard_iron[1]==8.5 and b.soft_iron[1][1]==2)
      m:calibration_clear();assert(not m:calibration_get().calibrated)
      m:calibration_reset();for i=1,16 do m:calibration_add_sample({0,i,i}) end
      assert(not pcall(m.calibration_finish,m));m:close()
    "#,
    );
    finish(&mut vm);
}

#[test]
fn hardware_arguments_reject_lossy_coercion_and_external_resources() {
    let mut vm = vm(
        Board::present(),
        r#"
      local imu=require('imu');local mag=require('magnetometer');local led=require('led_strip');local gauge=require('fuel_gauge')
      assert(not pcall(imu.new,{}));assert(not pcall(imu.new,{device='imu_sensor',bus={}}))
      assert(not pcall(imu.new,{device='imu_sensor'},2));assert(not pcall(mag.new,{chip='bmm150'}))
      assert(not pcall(gauge.new,{bus={}}));assert(not pcall(gauge.new,{chip='max17048'}))
      assert(not pcall(gauge.new,{port='0'}));assert(not pcall(gauge.new,{borrow=false}))
      for _,n in ipairs({0,7,16,1.5,math.huge,-1}) do assert(not pcall(led.new,38,n)) end
      assert(not pcall(led.new,15,6));assert(not pcall(led.new,'38',6))
      local s=led.new(38,6)
      assert(not pcall(s.set_pixel,s,6,0,0,0));assert(not pcall(s.set_pixel,s,0,-1,0,0))
      assert(not pcall(s.set_pixel,s,0,256,0,0));assert(not pcall(s.set_pixel_hsv,s,0,360,255,255))
      assert(not pcall(s.set_pixel_hsv,s,0,0,0/0,255));s:close()
      assert(not pcall(require('haptic').play,'arbitrary'));assert(not pcall(require('haptic').play,1))
    "#,
    );
    finish(&mut vm);
}

#[test]
fn output_requests_are_staged_bounded_and_released() {
    let mut vm = vm(
        Board::present(),
        r#"
      local led=require('led_strip');local s=led.new(38,6)
      s:set_pixel(0,255,0,0);s:set_pixel_hsv(5,120,255,64)
      require('delay').delay_ms(1)
      s:refresh();s:clear();s:close();s:close()
      local h=require('haptic');for _,p in ipairs(h.presets()) do h.play(p) end
    "#,
    );
    sleep(&mut vm);
    assert!(vm.host_mut::<Board>().unwrap().requests.is_empty());
    finish(&mut vm);
    let requests = &vm.host_mut::<Board>().unwrap().requests;
    let HardwareRequest::LedFrame(frame) = requests[0] else {
        panic!("expected frame")
    };
    assert_eq!(frame[0], [255, 0, 0]);
    assert_eq!(frame[5], [0, 64, 0]);
    assert_eq!(requests[1], HardwareRequest::LedFrame([[0; 3]; 6]));
    assert_eq!(requests[2], HardwareRequest::LedRelease);
    assert_eq!(requests.len(), 16);
}

#[test]
fn output_errors_keep_the_real_host_error_and_disconnect_invalidates_handle() {
    let mut board = Board::present();
    board.reject = true;
    let mut vm = vm(
        board,
        r#"
      local s=require('led_strip').new(38,6)
      local ok,err=pcall(s.refresh,s);assert(not ok and err:find('262',1,true),err)
      require('delay').delay_ms(1)
      ok,err=pcall(s.refresh,s);assert(not ok and err:find('board changed',1,true),err)
    "#,
    );
    sleep(&mut vm);
    vm.host_mut::<Board>().unwrap().snapshot.led_generation += 1;
    finish(&mut vm);
}

#[test]
fn button_domain_defaults_and_explicit_click_gap_boundaries() {
    for (short, gap, doubles, singles) in [
        (0, 399, 1, 0),
        (0, 400, 0, 2),
        (0, 401, 0, 2),
        (180, 179, 1, 0),
        (180, 180, 0, 2),
        (180, 399, 0, 2),
    ] {
        let mut board = Board::present();
        board.snapshot.onboard.ready = 1;
        let source = alloc::format!(
            r#"
          local b=require('button');local d=require('delay')
          local h=assert(b.new(11,0,0,{short}))
          assert(h.short=={effective})
          local other=assert(b.new(10));assert(other.short==400)
          local ai=assert(b.new(7));assert(ai.short==180)
          local boot=assert(b.new(61,0,0,0));assert(boot.short==180)
          local singles,doubles=0,0
          b.on(h,'single_click',function() singles=singles+1 end)
          b.on(h,'double_click',function() doubles=doubles+1 end)
          d.delay_ms(2000);b.dispatch()
          assert(singles=={singles} and doubles=={doubles})
        "#,
            effective = if short == 0 { 400 } else { short },
        );
        let mut vm = vm(board, &source);
        sleep(&mut vm);
        let s = &mut vm.host_mut::<Board>().unwrap().snapshot;
        s.button_edge(0, true, 1100);
        s.button_edge(0, false, 1140);
        s.button_edge(0, true, 1140 + gap);
        s.button_edge(0, false, 1180 + gap);
        finish(&mut vm);
    }
}

#[test]
fn button_default_long_press_does_not_join_following_short_click() {
    let mut vm = vm(
        Board::present(),
        r#"
      local b=require('button');local h=assert(b.new(11))
      local singles,doubles,longs=0,0,0
      b.on(h,'single_click',function() singles=singles+1 end)
      b.on(h,'double_click',function() doubles=doubles+1 end)
      b.on(h,'long_press_up',function(e) longs=longs+1;assert(e.pressed_time_ms==1500) end)
      require('delay').delay_ms(3000);b.dispatch()
      assert(singles==1 and doubles==0 and longs==1)
    "#,
    );
    sleep(&mut vm);
    let s = &mut vm.host_mut::<Board>().unwrap().snapshot;
    s.button_edge(0, true, 1100);
    s.button_edge(0, false, 2600);
    s.button_edge(0, true, 2700);
    s.button_edge(0, false, 2740);
    finish(&mut vm);
}

#[test]
fn button_taps_between_dispatches_keep_edge_timing_and_callbacks_are_lua_only() {
    let mut vm = vm(
        Board::present(),
        r#"
      local b=require('button');local d=require('delay');local h=assert(b.new(11,0,500,100))
      assert(b.get_key_level(h)==1);local log={}
      for _,event in ipairs({'press_down','press_up','double_click','single_click'}) do
        b.on(h,event,function(e) log[#log+1]=e end)
      end
      d.delay_ms(1000);assert(#log==0);assert(b.dispatch()==5)
      assert(log[1].event=='press_down' and log[2].event=='press_up' and log[2].pressed_time_ms==40)
      assert(log[4].pressed_time_ms==30 and log[5].event=='double_click' and log[5].repeat_count==2)
      assert(b.dispatch()==0);assert(b.close(h));assert(b.close(h))
    "#,
    );
    sleep(&mut vm);
    let s = &mut vm.host_mut::<Board>().unwrap().snapshot;
    s.button_edge(0, true, 1100);
    s.button_edge(0, false, 1140);
    s.button_edge(0, true, 1190);
    s.button_edge(0, false, 1220);
    finish(&mut vm);
}

#[test]
fn button_long_press_reentrancy_close_and_board_generation() {
    let mut vm = vm(
        Board::present(),
        r#"
      local b=require('button');local d=require('delay');local h=assert(b.new(11,0,500,100))
      local duration
      b.on(h,'long_press_start',function(e) duration=e.pressed_time_ms end)
      b.on(h,'long_press_up',function(e) assert(e.pressed_time_ms==800);assert(not pcall(b.dispatch));b.close(h) end)
      b.on(h,'press_end',function() error('callback survived close') end)
      d.delay_ms(1000);b.dispatch();assert(duration==800)
      h=assert(b.new(11));b.on(h,'press_down',function() error('stale callback') end)
      d.delay_ms(1);assert(b.get_key_level(h)==nil);assert(b.dispatch()==0)
      assert(not pcall(b.get_key_level,h));assert(b.new(7)==nil)
    "#,
    );
    sleep(&mut vm);
    let s = &mut vm.host_mut::<Board>().unwrap().snapshot;
    s.button_edge(0, true, 1100);
    s.button_edge(0, false, 1900);
    sleep(&mut vm);
    let s = &mut vm.host_mut::<Board>().unwrap().snapshot;
    s.button_board(Some([13, 12]));
    s.valid |= 64;
    finish(&mut vm);
}

#[test]
fn button_queue_overflow_is_explicit_and_dispatch_recovers_after_callback_error() {
    let mut vm = vm(
        Board::present(),
        r#"
      local b=require('button');local d=require('delay');local h=assert(b.new(11))
      b.on(h,'press_down',function() error('callback exploded') end)
      d.delay_ms(100);local ok,e=pcall(b.dispatch);assert(not ok and e:find('callback exploded',1,true));assert(b.dispatch()==0)
      d.delay_ms(100);ok,e=pcall(b.dispatch);assert(not ok and e:find('overflow',1,true))
      b.close(h);assert(b.dispatch()==0)
    "#,
    );
    sleep(&mut vm);
    vm.host_mut::<Board>()
        .unwrap()
        .snapshot
        .button_edge(0, true, 1001);
    sleep(&mut vm);
    for i in 0..18 {
        vm.host_mut::<Board>()
            .unwrap()
            .snapshot
            .button_edge(0, i % 2 == 0, 1101 + i);
    }
    finish(&mut vm);
}

#[test]
fn official_button_script_runs_with_board_gpio_argument() {
    let mut vm = vm(
        Board::present(),
        include_str!("../vendor/esp-claw/hardware-contracts/button/test/button_events.lua"),
    );
    vm.set_args_json(r#"{"pin":11,"duration_ms":10,"poll_ms":1}"#)
        .unwrap();
    finish(&mut vm);
    assert!(vm
        .host_mut::<Board>()
        .unwrap()
        .lines
        .iter()
        .any(|s| s.contains("[button_demo] done")));
}

#[test]
fn official_led_script_requires_the_explicit_six_pixel_board_adaptation() {
    let source =
        include_str!("../vendor/esp-claw/hardware-contracts/led_strip/test/led_strip_rainbow.lua");
    let mut original = vm(Board::present(), source);
    loop {
        match original.resume() {
            Step::Slice => {}
            Step::Error(error) => {
                assert!(error.contains("outside allowed range"), "{error}");
                break;
            }
            other => panic!("unadapted 16-LED script unexpectedly ran: {other:?}"),
        }
    }
    let adapted = source.replace("local LED_COUNT = 16", "local LED_COUNT = 6");
    let mut adapted = vm(Board::present(), &adapted);
    finish(&mut adapted);
    assert!(adapted
        .host_mut::<Board>()
        .unwrap()
        .requests
        .iter()
        .any(|r| matches!(r, HardwareRequest::LedFrame(_))));
}

#[test]
fn fuel_board_example_and_external_bus_contract_are_explicit() {
    let mut vm = vm(
        Board::present(),
        r#"
      local g=require('lib_fuel_gauge').new({borrow=true,port=0,chip='bq27220'})
      assert(g:read_soc()==100 and g:read_voltage_mv()==4200 and g:read_current_ma()==120);g:close()
      local ok,e=pcall(require,'i2c');assert(not ok and e:find("not found",1,true))
      assert(not pcall(require('lib_fuel_gauge').new,{bus={}}))
    "#,
    );
    finish(&mut vm);
}

#[test]
fn led_release_is_observable_after_gc_vm_drop_and_error() {
    for ending in [
        "s=nil;collectgarbage('collect')",
        "require('delay').delay_ms(1)",
        "error('script failed')",
    ] {
        let shared = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut board = Board::present();
        board.shared = Some(shared.clone());
        let source = alloc::format!("local s=require('led_strip').new(38,6);s:refresh();{ending}");
        let mut vm = vm(board, &source);
        loop {
            match vm.resume() {
                Step::Slice => {}
                Step::Done | Step::Error(_) | Step::Request(Request::Sleep { .. }) => break,
                other => panic!("unexpected {other:?}"),
            }
        }
        drop(vm);
        let requests = shared.lock().unwrap();
        assert_eq!(
            requests.as_slice(),
            &[
                HardwareRequest::LedFrame([[0; 3]; 6]),
                HardwareRequest::LedRelease
            ],
            "{ending}"
        );
    }
}

#[test]
fn imu_requires_auxiliary_data_instead_of_returning_partial_sample() {
    let mut board = Board::present();
    board.snapshot.valid &= !8;
    let mut vm = vm(
        board,
        r#"
      local s=require('imu').new({device='imu_sensor'})
      local ok,e=pcall(s.read,s);assert(not ok and e:find('sample unavailable',1,true));s:close()
    "#,
    );
    finish(&mut vm);
}

#[test]
fn button_thresholds_use_zero_defaults_and_uint16_boundaries() {
    let mut vm = vm(
        Board::present(),
        r#"
      local b=require('button')
      for _,cfg in ipairs({{0,0},{1,65535},{65535,1},{65535,65535}}) do
        local h=assert(b.new(11,0,cfg[1],cfg[2]));b.close(h)
      end
      for _,n in ipairs({-1,65536,1.5,math.huge}) do
        assert(not pcall(b.new,11,0,n));assert(not pcall(b.new,11,0,500,n))
      end
      local h=assert(b.new(11.0));b.close(h)
    "#,
    );
    finish(&mut vm);
}

#[test]
fn onboard_button_domains_survive_slot_detach_and_reinsert_with_real_lua_callbacks() {
    let mut board = Board::present();
    board.snapshot.onboard.ready = 1;
    let mut vm = vm(
        board,
        r#"
        local b=require('button'); local d=require('delay')
        local ai=assert(b.new(7)); local boot=assert(b.new(61)); local slot=assert(b.new(11))
        local short,double,long=0,0,0
        b.on(ai,'single_click',function() short=short+1 end)
        b.on(boot,'double_click',function() double=double+1 end)
        b.on(boot,'long_press_start',function(e) assert(e.pressed_time_ms>=1500);long=long+1 end)
        d.delay_ms(1000)
        assert(b.get_key_level(slot)==nil)
        assert(b.get_key_level(ai)==1 and b.get_key_level(boot)==1)
        b.dispatch(); assert(short==1 and double==1 and long==0)
        d.delay_ms(2000)
        assert(b.get_key_level(ai)==1); b.dispatch(); assert(long==1)
        d.delay_ms(1000)
        b.dispatch(); assert(long==1 and short==1 and double==1)
        assert(b.close(ai));assert(b.close(boot))
    "#,
    );
    sleep(&mut vm);
    let s = &mut vm.host_mut::<Board>().unwrap().snapshot;
    s.button_board(None);
    s.onboard.edge(0, true, 1100);
    s.onboard.edge(0, false, 1140);
    s.onboard.edge(1, true, 1200);
    s.onboard.edge(1, false, 1240);
    s.onboard.edge(1, true, 1300);
    s.onboard.edge(1, false, 1340);
    sleep(&mut vm);
    let s = &mut vm.host_mut::<Board>().unwrap().snapshot;
    s.button_board(Some([13, 12]));
    s.valid |= 64;
    s.onboard.edge(1, true, 2100);
    sleep(&mut vm);
    let s = &mut vm.host_mut::<Board>().unwrap().snapshot;
    s.onboard.edge(1, false, 4100);
    finish(&mut vm);
}

#[test]
fn onboard_open_held_does_not_invent_long_or_click_and_defaults_stay_low_active() {
    let mut board = Board::default();
    board.snapshot.onboard.ready = 1;
    board.snapshot.onboard.pressed[0] = 1;
    let mut vm = vm(
        board,
        r#"
        local b=require('button');local d=require('delay')
        local ai=assert(b.new(7));assert(b.get_key_level(ai)==0)
        b.on(ai,'long_press_start',function() error('invented initial long') end)
        b.on(ai,'single_click',function() error('invented initial click') end)
        d.delay_ms(3000);b.dispatch()
        d.delay_ms(1000);b.dispatch();assert(b.get_key_level(ai)==1)
        assert(b.close(ai));assert(b.new(11)==nil)
    "#,
    );
    sleep(&mut vm);
    sleep(&mut vm);
    vm.host_mut::<Board>()
        .unwrap()
        .snapshot
        .onboard
        .edge(0, false, 4100);
    finish(&mut vm);
}
