use alloc::{format, string::String, vec::Vec};
use esp_hal::peripherals::WIFI;
use esp_radio::wifi::{
    scan::ScanConfig, sta::StationConfig, AuthenticationMethodConfig, Config, ControllerConfig,
    Interface, PowerSaveMode, WifiController, WifiError,
};
use os::radio::Credentials;

pub struct Network {
    pub ssid: Vec<u8>,
    pub channel: u8,
    pub rssi: i8,
    pub auth: String,
}
pub struct Station<'d>(WifiController<'d>);
impl<'d> Station<'d> {
    pub fn new(device: WIFI<'d>) -> Result<Self, String> {
        let mut controller = WifiController::new(device, ControllerConfig::default())
            .map_err(|e| format!("Wi-Fi init {e:?}"))?;
        // S31 retention callbacks are incomplete in the pinned upstream.
        // Keep modem sleep disabled until that path has its own acceptance.
        controller
            .set_power_saving(PowerSaveMode::None)
            .map_err(|e| format!("Wi-Fi power mode {e:?}"))?;
        Ok(Self(controller))
    }
    pub fn connected(&self) -> bool {
        self.0.is_connected()
    }
    pub fn configure(&mut self, credentials: &Credentials) -> Result<(), String> {
        let config = StationConfig::default()
            .with_ssid(
                credentials
                    .ssid
                    .as_slice()
                    .try_into()
                    .map_err(|_| "SSID rejected")?,
            )
            .with_authentication(AuthenticationMethodConfig::Wpa2Personal(
                credentials
                    .password
                    .as_slice()
                    .try_into()
                    .map_err(|_| "password rejected")?,
            ));
        self.0
            .set_config(&Config::Station(config))
            .map_err(|e| format!("Wi-Fi configuration {e:?}"))
    }
    pub async fn scan(&mut self) -> Result<Vec<Network>, String> {
        let results = self
            .0
            .scan_async(&ScanConfig::default().with_max(12))
            .await
            .map_err(|e| format!("Wi-Fi scan {e:?}"))?;
        Ok(results
            .into_iter()
            .map(|ap| Network {
                ssid: ap.ssid.as_bytes().into(),
                channel: ap.channel,
                rssi: ap.signal_strength,
                auth: format!("{:?}", ap.auth_method),
            })
            .collect())
    }
    pub async fn connect(&mut self) -> Result<(), String> {
        self.0
            .connect_async()
            .await
            .map(|_| ())
            .map_err(|e| match e {
                esp_radio::wifi::ConnectionError::Failed(info) => {
                    format!("Wi-Fi association {:?}", info.reason)
                }
                _ => "Wi-Fi association driver error".into(),
            })
    }
    pub async fn disconnect(&mut self) -> Result<(), String> {
        match self.0.disconnect_async().await {
            // The service's disconnect contract also cancels desired retries.
            // An already disconnected controller has reached that state.
            Ok(_) | Err(WifiError::NotConnected) => Ok(()),
            Err(e) => Err(format!("Wi-Fi disconnect {e:?}")),
        }
    }
}

pub fn network() -> (
    embassy_net::Stack<'static>,
    embassy_net::Runner<'static, Interface>,
) {
    static RESOURCES: static_cell::StaticCell<embassy_net::StackResources<4>> =
        static_cell::StaticCell::new();
    let rng = esp_hal::rng::Rng::new();
    let seed = (u64::from(rng.random()) << 32) | u64::from(rng.random());
    embassy_net::new(
        Interface::station(),
        embassy_net::Config::dhcpv4(Default::default()),
        RESOURCES.init(embassy_net::StackResources::new()),
        seed,
    )
}
