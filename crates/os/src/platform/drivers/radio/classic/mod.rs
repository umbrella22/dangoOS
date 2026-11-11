//! Classic BR/EDR HCI transport; no BLE or GATT host is instantiated.
use alloc::{format, string::String};
use esp_radio::ble::{controller::BleConnector, Config};
mod runtime;
mod storage;
mod timers;

pub fn controller(device: esp_hal::peripherals::BT<'_>) -> Result<BleConnector<'_>, String> {
    BleConnector::new(
        device,
        Config::default()
            .with_classic(true)
            .with_modem_sleep(false)
            .with_max_connections(1),
    )
    .map_err(|e| format!("Classic controller {e:?}"))
}
pub fn init_adapters() {
    timers::init();
}
