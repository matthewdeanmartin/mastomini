//! Wi-Fi for the bots board: the household network from the build
//! (`MASTOMINI_WIFI_SSID` / `_PASSWORD`, see build.rs), joined with retries.
//!
//! Unlike a single attempt at boot, this keeps trying: after a power cut the
//! router usually comes back slower than the board. There is no setup
//! network in this proof of concept; a board without credentials says so in
//! its log and waits.

use esp_idf_svc::wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi};
use std::net::Ipv4Addr;
use std::time::Duration;

pub struct Net {
    wifi: BlockingWifi<EspWifi<'static>>,
}

impl Net {
    pub fn new(wifi: BlockingWifi<EspWifi<'static>>) -> Net {
        Net { wifi }
    }

    fn try_join(&mut self, ssid: &str, password: &str) -> Result<Ipv4Addr, String> {
        let conf = ClientConfiguration {
            ssid: ssid.try_into().map_err(|_| "SSID too long")?,
            password: password.try_into().map_err(|_| "password too long")?,
            auth_method: if password.is_empty() {
                AuthMethod::None
            } else {
                AuthMethod::WPA2Personal
            },
            ..Default::default()
        };
        self.wifi
            .set_configuration(&Configuration::Client(conf))
            .map_err(|e| e.to_string())?;
        if !self.wifi.is_started().unwrap_or(false) {
            self.wifi.start().map_err(|e| e.to_string())?;
        }
        self.wifi.connect().map_err(|e| e.to_string())?;
        self.wifi.wait_netif_up().map_err(|e| e.to_string())?;
        let info = self
            .wifi
            .wifi()
            .sta_netif()
            .get_ip_info()
            .map_err(|e| e.to_string())?;
        Ok(info.ip)
    }

    /// Join, retrying with growing pauses (5 s up to a minute) until it works.
    pub fn join(&mut self, ssid: &str, password: &str) -> Ipv4Addr {
        let mut pause = 5;
        loop {
            match self.try_join(ssid, password) {
                Ok(ip) => return ip,
                Err(e) => {
                    log::warn!("Could not join {ssid} ({e}); trying again in {pause} s");
                    let _ = self.wifi.disconnect();
                    std::thread::sleep(Duration::from_secs(pause));
                    pause = (pause * 2).min(60);
                }
            }
        }
    }

    /// Reconnect if the router went away. Called every 30 s.
    pub fn keep_connected(&mut self) {
        if self.wifi.is_connected().unwrap_or(true) {
            return;
        }
        log::warn!("Wi-Fi disconnected; reconnecting");
        if let Err(e) = self.wifi.connect().and_then(|_| self.wifi.wait_netif_up()) {
            log::warn!("Reconnect: {e}");
        }
    }
}
