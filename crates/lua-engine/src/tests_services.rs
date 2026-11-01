//! Public Lua contracts run through the actual VM and C/Rust bridge.
use crate::*;
use alloc::{boxed::Box, vec::Vec};
use core::alloc::Layout;

#[derive(Default)]
struct Board {
    heap: HeapInfo,
    services: ServiceSnapshot,
    hardware: HardwareSnapshot,
    controls: Vec<AudioControl>,
    probes: Vec<(u32, Layout)>,
    reject: Option<i32>,
}
impl Host for Board {
    fn print(&mut self, _: &[u8]) {}
    fn heap_info(&mut self, _: u32) -> HeapInfo {
        self.heap
    }
    fn heap_probe(&mut self, caps: u32, layout: Layout) -> Result<bool, i32> {
        self.probes.push((caps, layout));
        self.reject.map_or(Ok(layout.size() <= 4096), Err)
    }
    fn hardware_snapshot(&mut self) -> HardwareSnapshot {
        self.hardware
    }
    fn service_snapshot(&mut self) -> ServiceSnapshot {
        self.services
    }
    fn audio_control(&mut self, request: AudioControl) -> Result<(), i32> {
        if let Some(error) = self.reject {
            return Err(error);
        }
        if !self.services.audio.is_some_and(|s| s.controls_available) {
            return Err(esp_err::INVALID_STATE);
        }
        self.controls.push(request);
        Ok(())
    }
}
fn vm(board: Board) -> Vm {
    Vm::new(
        Box::new(board),
        Limits {
            memory_bytes: 256 * 1024,
            ..Limits::default()
        },
        &GLOBAL_ALLOCATOR,
        9,
    )
    .unwrap()
}
fn finish(vm: &mut Vm, source: &str) {
    vm.load("services-test", source.as_bytes()).unwrap();
    for _ in 0..1000 {
        match vm.resume() {
            Step::Done => return,
            Step::Slice => (),
            other => panic!("unexpected result {other:?}"),
        }
    }
    panic!("test script did not finish");
}
fn board() -> Board {
    Board {
        services: ServiceSnapshot {
            audio: Some(AudioStatus {
                available: true,
                controls_available: true,
                state: "ready",
                sample_rate_hz: 16000,
                volume: 0,
                output_ceiling_db: -15,
                muted: false,
                tx_frames: 0,
                rx_frames: 0,
                underruns: 0,
                overruns: 0,
                peak: 0,
                error: None,
            }),
            camera: Some(CameraStatus {
                available: false,
                state: "off",
                sensor: "unknown",
                width: 0,
                height: 0,
                frames: 0,
                dropped: 0,
                saved: 0,
                error: None,
            }),
            network: Some(NetworkStatus {
                wifi_state: "off",
                bluetooth_state: "off",
                ip: None,
            }),
            native_ui: Some(NativeUiLimits {
                objects: 512,
                image_bytes: 2 * 1024 * 1024,
            }),
        },
        ..Board::default()
    }
}

#[test]
fn heap_unavailable_and_measured_zero_survive_the_c_boundary() {
    let mut vm = vm(Board {
        heap: HeapInfo {
            free_size: 0,
            total_size: 1000,
            allocated_size: 1000,
            largest_free_block: 0,
            minimum_free_size: 999,
            sampled_minimum_free_size: 0,
            allocated_blocks: 7,
            free_blocks: 0,
            total_blocks: 7,
            valid_fields: heap_fields::SIZES
                | heap_fields::LARGEST_FREE
                | heap_fields::BLOCK_COUNTS
                | heap_fields::SAMPLED_MINIMUM_FREE,
        },
        ..Board::default()
    });
    finish(
        &mut vm,
        r#"
      local s=require('system'); local h=s.heap.get_info()
      assert(h.free_size==0 and h.largest_free_block==0 and h.largest_free_block_alignment==1)
      assert(h.minimum_free_size==nil and h.sampled_minimum_free_size==0)
      assert(h.allocated_blocks==7 and h.free_blocks==0 and h.total_blocks==7)
      assert(s.info().sram_largest==0)
    "#,
    );
    let mut vm = self::vm(Board::default());
    finish(
        &mut vm,
        r#"
      local s=require('system'); local h=s.heap.get_info()
      assert(h.total_size==nil and h.free_size==nil and h.allocated_size==nil)
      assert(h.largest_free_block==nil and h.minimum_free_size==nil and h.total_blocks==nil)
      assert(s.info().sram_largest==nil and s.info().sram_free==nil)
    "#,
    );
}

#[test]
fn allocation_probes_preserve_layout_and_distinguish_failure_from_unsupported() {
    let mut vm = vm(Board::default());
    finish(
        &mut vm,
        r#"
      local h=require('system').heap
      assert(h.probe(h.caps.SPIRAM,4096,128)==true)
      assert(h.probe(h.caps.INTERNAL,8192,16)==false)
      assert(not pcall(h.probe,h.caps.DEFAULT,10,16))
      assert(not pcall(h.probe,h.caps.SPIRAM,0,16))
      assert(not pcall(h.probe,h.caps.SPIRAM,'4096',128))
      assert(not pcall(h.probe,h.caps.SPIRAM,4096,'128'))
      local ok,code=h.probe(h.caps.SPIRAM,10,3)
      assert(ok==nil and code==258)
    "#,
    );
    let board = vm.host_mut::<Board>().unwrap();
    assert_eq!(
        board.probes,
        [
            (caps::SPIRAM, Layout::from_size_align(4096, 128).unwrap()),
            (caps::INTERNAL, Layout::from_size_align(8192, 16).unwrap())
        ]
    );
    let mut vm = self::vm(Board {
        reject: Some(esp_err::NOT_SUPPORTED),
        ..Board::default()
    });
    finish(&mut vm, "local h=require('system').heap;local ok,e=h.probe(h.caps.SPIRAM,10,16);assert(ok==nil and e==262)");
}

#[test]
fn capabilities_report_host_support_availability_and_the_actual_vm_limit() {
    let mut b = board();
    b.hardware.available = 1;
    let mut vm = vm(b);
    finish(
        &mut vm,
        r#"
      local c=require('capabilities').get()
      assert(c.api_version==1 and c.modules.imu.available)
      assert(c.modules.audio.supported and c.modules.audio.available and c.audio_controls_allowed)
      assert(c.modules.camera.supported and not c.modules.camera.available)
      assert(c.modules.network.supported and not c.modules.network.available)
      assert(c.limits.lua_heap_bytes==256*1024 and c.limits.native_ui.objects==512)
      assert(c.limits.native_ui.image_bytes==2*1024*1024)
      assert(#c.operations.audio==3 and c.operations.audio[1]=='status')
      assert(require('audio').status().volume==0 and require('camera').status().width==0)
      assert(require('audio').status().output_ceiling_db==-15)
      assert(require('network').status().ip==nil)
      assert(require('audio').play==nil and require('camera').capture==nil)
    "#,
    );
    let mut vm = self::vm(Board::default());
    finish(
        &mut vm,
        r#"
      local c=require('capabilities').get()
      assert(not c.modules.audio.supported and not c.audio_controls_allowed)
      assert(c.operations.audio==nil)
      assert(c.limits.native_ui==nil and not c.modules.lvgl.supported)
      assert(not pcall(require('audio').status))
    "#,
    );
}

#[test]
fn audio_controls_validate_before_mutation_and_return_the_real_host_error() {
    let mut vm = vm(board());
    finish(
        &mut vm,
        r#"
      local a=require('audio')
      a.set_volume(0);a.set_volume(100.0);a.set_muted(false)
      for _,v in ipairs({'20',-1,101,1.5,math.huge,0/0,true,{}}) do assert(not pcall(a.set_volume,v)) end
      for _,v in ipairs({0,1,'false',{}}) do assert(not pcall(a.set_muted,v)) end
      assert(not pcall(a.set_volume,50,60))
    "#,
    );
    assert_eq!(
        vm.host_mut::<Board>().unwrap().controls,
        [
            AudioControl::Volume(0),
            AudioControl::Volume(100),
            AudioControl::Muted(false)
        ]
    );
    let mut b = board();
    b.reject = Some(esp_err::TIMEOUT);
    let mut vm = self::vm(b);
    finish(
        &mut vm,
        "local ok,e=pcall(require('audio').set_volume,50);assert(not ok and e:find('263',1,true))",
    );
    assert!(vm.host_mut::<Board>().unwrap().controls.is_empty());
}

#[test]
fn restricted_callbacks_cannot_change_audio_through_saved_host_functions() {
    let mut vm = vm(board());
    vm.set_callback_budget(1000, 100000);
    finish(
        &mut vm,
        r#"
      assert(not require('capabilities').get().audio_controls_allowed)
      assert(not require('audio').status().controls_available)
      assert(not pcall(require('audio').set_volume,50))
      local set_muted = require('audio').set_muted
      assert(not pcall(set_muted,true))
    "#,
    );
    assert!(vm.host_mut::<Board>().unwrap().controls.is_empty());
}

#[test]
fn workers_have_neither_service_globals_nor_service_imports() {
    let mut vm = vm(board());
    vm.restrict_worker().unwrap();
    finish(
        &mut vm,
        r#"
      assert(capabilities==nil and audio==nil and camera==nil and network==nil)
      for _,name in ipairs({'capabilities','audio','camera','network'}) do
        assert(not pcall(require,name))
      end
    "#,
    );
    assert!(vm.host_mut::<Board>().unwrap().controls.is_empty());
}
