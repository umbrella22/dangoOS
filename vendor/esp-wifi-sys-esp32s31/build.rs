use std::{env, path::PathBuf, process::Command};

fn main() {
    // Put the linker script somewhere the linker can find it
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());

    let wifi = env::var_os("CARGO_FEATURE_WIFI").is_some();
    let bt = env::var_os("CARGO_FEATURE_BT").is_some();
    let ieee802154 = env::var_os("CARGO_FEATURE_IEEE802154").is_some();

    let libs = [
        ("ble_app", bt),
        ("bredr_app", bt),
        ("btdm_common", bt),
        ("btbb", bt || ieee802154),
        ("coexist", wifi || bt || ieee802154),
        ("core", wifi),
        ("espnow", wifi),
        ("mesh", wifi),
        ("net80211", wifi),
        ("phy", true),
        ("pp", wifi),
        ("smartconfig", wifi),
        ("wapi", wifi),
        ("wpa_supplicant", wifi),
        ("printf", true),
        ("regulatory", wifi),
    ];

    for (lib, _) in libs.into_iter().filter(|(_, enabled)| *enabled) {
        std::fs::copy(
            format!("libs/lib{}.a", lib),
            out.join(format!("lib{}.a", lib)),
        )
        .unwrap_or_else(|e| panic!("Failed to copy the {lib} library: {e}"));
        {
            // The blob's logging entry points share an object with a small C
            // formatter. Keep that formatter private: Lua links full newlib
            // (including floating-point/locale formatting) in the same image.
            // Rewrite definitions AND relocations; never permit duplicate
            // definitions or let link order choose Lua's formatting behavior.
            let target = env::var("TARGET").unwrap().replace('-', "_");
            let objcopy = env::var_os("OBJCOPY")
                .map(PathBuf::from)
                .or_else(|| {
                    env::var_os(format!("CC_{target}"))
                        .map(|cc| PathBuf::from(cc).with_file_name("riscv32-esp-elf-objcopy"))
                })
                .unwrap_or_else(|| "riscv32-esp-elf-objcopy".into());
            let mut command = Command::new(objcopy);
            // These blob calls must use esp-radio's scheduler/boot-clock
            // adapters. Its PROVIDE aliases lose to full newlib definitions.
            for symbol in ["gettimeofday", "usleep", "sleep"] {
                command
                    .arg("--redefine-sym")
                    .arg(format!("{symbol}=__esp_radio_{symbol}"));
            }
            if lib == "printf" {
                for symbol in ["printf", "sprintf", "snprintf", "vsnprintf", "vprintf"] {
                    command
                        .arg("--redefine-sym")
                        .arg(format!("{symbol}=mosaico_radio_{symbol}"));
                }
            }
            if lib == "bredr_app" {
                // Route fatal assertions into the firmware's retained panic
                // record. The stock handler writes UART and then faults at 0,
                // losing its file/line on boards exposing only USB CDC.
                command.arg("--weaken-symbol=r_co_assert_info");
            }
            let status = command
                .arg(out.join(format!("lib{lib}.a")))
                .status()
                .expect("run RISC-V objcopy");
            assert!(status.success(), "namespace the radio C integration");
        }
        println!("cargo:rustc-link-lib={lib}");
    }

    println!("cargo:rustc-link-search={}", out.display());
}
