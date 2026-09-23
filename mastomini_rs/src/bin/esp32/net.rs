//! Wi-Fi: joining the household network, and the setup network for boards
//! that have no working Wi-Fi yet.
//!
//! A board with no saved network (or one it cannot join) opens the open
//! network `mastomini-setup`. A tiny DNS responder answers every name with the
//! board's address, so phones show the setup page as a captive portal. The
//! page lists nearby networks; the board joins the chosen one while keeping the
//! setup network up, shows its new address, and saves the credentials in the
//! system NVS partition. The setup network closes shortly after the
//! household account has been created (spec/06).

use esp_idf_svc::{
    nvs::{EspNvs, NvsDefault},
    sys::EspError,
    wifi::{
        AccessPointConfiguration, AuthMethod, BlockingWifi, ClientConfiguration, Configuration,
        EspWifi,
    },
};
use mastomini::api::html;
use mastomini::http::{Params, Request, Response};
use mastomini::text::escape;
use std::net::{Ipv4Addr, UdpSocket};
use std::time::{Duration, Instant};

use mastomini::captive::dns_reply;
pub use mastomini::captive::{captive, AP_IP};

pub const AP_SSID: &str = "mastomini-setup";
/// Saved Wi-Fi credentials, in the system `nvs` partition (not `store`).
pub const NAMESPACE: &str = "mm_wifi";
/// How long the setup network stays up once the household exists.
const CLOSE_AFTER_PROVISIONED: Duration = Duration::from_secs(120);
/// Or once Wi-Fi works, even if nobody creates the household.
const CLOSE_AFTER_JOINED: Duration = Duration::from_secs(20 * 60);
const JOIN_TIMEOUT: Duration = Duration::from_secs(20);

pub struct Net {
    wifi: BlockingWifi<EspWifi<'static>>,
    nvs: EspNvs<NvsDefault>,
    pub setup_mode: bool,
    pub ssid: String,
    password: String,
    pub sta_ip: Option<Ipv4Addr>,
    joined_at: Option<Instant>,
    provisioned_at: Option<Instant>,
    /// Why setup mode started, shown on the page.
    reason: Option<String>,
}

fn client(ssid: &str, password: &str) -> Result<ClientConfiguration, String> {
    Ok(ClientConfiguration {
        ssid: ssid
            .try_into()
            .map_err(|_| "The network name is too long")?,
        password: password
            .try_into()
            .map_err(|_| "The password is too long")?,
        auth_method: if password.is_empty() {
            AuthMethod::None
        } else {
            AuthMethod::WPA2Personal
        },
        ..Default::default()
    })
}

fn access_point() -> AccessPointConfiguration {
    AccessPointConfiguration {
        ssid: AP_SSID.try_into().unwrap_or_default(),
        auth_method: AuthMethod::None,
        channel: 1,
        max_connections: 4,
        ..Default::default()
    }
}

impl Net {
    pub fn new(wifi: BlockingWifi<EspWifi<'static>>, nvs: EspNvs<NvsDefault>) -> Net {
        Net {
            wifi,
            nvs,
            setup_mode: false,
            ssid: String::new(),
            password: String::new(),
            sta_ip: None,
            joined_at: None,
            provisioned_at: None,
            reason: None,
        }
    }

    /// Saved credentials, else the developer's built-in ones.
    pub fn saved(&self) -> Option<(String, String, bool)> {
        let mut ssid = [0u8; 33];
        let mut pass = [0u8; 65];
        if let (Ok(Some(s)), Ok(p)) = (
            self.nvs.get_str("ssid", &mut ssid),
            self.nvs.get_str("pass", &mut pass),
        ) {
            return Some((s.to_string(), p.unwrap_or("").to_string(), true));
        }
        match (
            option_env!("MASTOMINI_WIFI_SSID"),
            option_env!("MASTOMINI_WIFI_PASSWORD"),
        ) {
            (Some(s), p) => Some((s.to_string(), p.unwrap_or("").to_string(), false)),
            _ => None,
        }
    }

    fn save(&self, ssid: &str, password: &str) -> Result<(), EspError> {
        self.nvs.set_str("ssid", ssid)?;
        self.nvs.set_str("pass", password)
    }

    fn sta_address(&self) -> Option<Ipv4Addr> {
        let info = self.wifi.wifi().sta_netif().get_ip_info().ok()?;
        (!info.ip.is_unspecified()).then_some(info.ip)
    }

    /// Normal boot: station only.
    pub fn join(&mut self, ssid: &str, password: &str, saved: bool) -> Result<Ipv4Addr, String> {
        let conf = client(ssid, password)?;
        self.wifi
            .set_configuration(&Configuration::Client(conf))
            .map_err(|e| e.to_string())?;
        if !self.wifi.is_started().unwrap_or(false) {
            self.wifi.start().map_err(|e| e.to_string())?;
        }
        self.wifi.connect().map_err(|e| e.to_string())?;
        self.wifi.wait_netif_up().map_err(|e| e.to_string())?;
        let ip = self.sta_address().ok_or("no address")?;
        if !saved {
            // Built-in developer credentials worked: keep them, so later
            // builds without them still find this network.
            if let Err(e) = self.save(ssid, password) {
                log::warn!("Could not save Wi-Fi credentials: {e}");
            }
        }
        self.ssid = ssid.to_string();
        self.password = password.to_string();
        self.sta_ip = Some(ip);
        Ok(ip)
    }

    /// Open the setup network (station side idle until a network is chosen).
    pub fn start_setup(&mut self, reason: Option<String>) -> Result<(), EspError> {
        let _ = self.wifi.disconnect();
        self.wifi.set_configuration(&Configuration::Mixed(
            ClientConfiguration::default(),
            access_point(),
        ))?;
        if !self.wifi.is_started()? {
            self.wifi.start()?;
        }
        self.setup_mode = true;
        self.reason = reason;
        Ok(())
    }

    /// Join the chosen network while keeping the setup network up.
    fn join_from_setup(&mut self, ssid: &str, password: &str) -> Result<Ipv4Addr, String> {
        let conf = client(ssid, password)?;
        let _ = self.wifi.disconnect();
        self.wifi
            .set_configuration(&Configuration::Mixed(conf, access_point()))
            .map_err(|e| e.to_string())?;
        // connect() waits for association; a wrong password fails here.
        if let Err(e) = self.wifi.connect() {
            let _ = self.wifi.disconnect();
            return Err(format!("could not connect ({e})"));
        }
        let deadline = Instant::now() + JOIN_TIMEOUT;
        while Instant::now() < deadline {
            if let Some(ip) = self.sta_address() {
                self.save(ssid, password).map_err(|e| e.to_string())?;
                self.ssid = ssid.to_string();
                self.password = password.to_string();
                self.sta_ip = Some(ip);
                self.joined_at = Some(Instant::now());
                log::info!("Ready at http://{ip}/ (http://{ip}/)");
                return Ok(ip);
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        let _ = self.wifi.disconnect();
        Err("joined, but the router gave no address".into())
    }

    /// Nearby network names, strongest first, without duplicates.
    fn scan(&mut self) -> Vec<String> {
        let mut found = self.wifi.scan().unwrap_or_default();
        found.sort_by_key(|ap| -i16::from(ap.signal_strength));
        let mut names: Vec<String> = Vec::new();
        for ap in found {
            let name = ap.ssid.to_string();
            if !name.is_empty() && name != AP_SSID && !names.contains(&name) {
                names.push(name);
            }
        }
        names.truncate(20);
        names
    }

    /// Close the setup network once it has done its job.
    pub fn maybe_close_setup(&mut self, provisioned: bool) {
        if !self.setup_mode {
            return;
        }
        let Some(joined) = self.joined_at else {
            return;
        };
        if provisioned && self.provisioned_at.is_none() {
            self.provisioned_at = Some(Instant::now());
        }
        let done = self
            .provisioned_at
            .is_some_and(|t| t.elapsed() > CLOSE_AFTER_PROVISIONED)
            || joined.elapsed() > CLOSE_AFTER_JOINED;
        if !done {
            return;
        }
        let Ok(conf) = client(&self.ssid, &self.password) else {
            return;
        };
        match self.wifi.set_configuration(&Configuration::Client(conf)) {
            Ok(()) => {
                self.setup_mode = false;
                log::info!("Setup network closed");
                if !self.wifi.is_connected().unwrap_or(false) {
                    let _ = self.wifi.connect();
                }
            }
            Err(e) => log::warn!("Could not close the setup network: {e}"),
        }
    }

    /// Station mode: reconnect if the router went away.
    pub fn keep_connected(&mut self) {
        if self.setup_mode || self.wifi.is_connected().unwrap_or(true) {
            return;
        }
        log::warn!("Wi-Fi disconnected; reconnecting");
        if let Err(e) = self.wifi.connect().and_then(|_| self.wifi.wait_netif_up()) {
            log::warn!("Reconnect: {e}");
        }
    }

    /// `GET`/`POST /setup/wifi`.
    pub fn page(&mut self, req: &Request, provisioned: bool) -> Response {
        if !self.setup_mode {
            return html::message(
                404,
                "Wi-Fi is set up",
                "This board is already on your network. If it ever cannot join it, it opens the mastomini-setup network again so you can choose a new one.",
            );
        }
        let params = Params::parse(req).unwrap_or_default();
        if req.method == "POST" {
            let ssid = params
                .text("ssid_other")
                .or_else(|| params.text("ssid"))
                .unwrap_or("")
                .trim()
                .to_string();
            let password = params.get("password").unwrap_or("").to_string();
            if ssid.is_empty() {
                return self.form(Some("Choose a network."), 422);
            }
            return match self.join_from_setup(&ssid, &password) {
                Ok(ip) => self.joined_page(ip, provisioned),
                Err(e) => {
                    log::warn!("Setup: joining {ssid} failed: {e}");
                    self.form(
                        Some(&format!(
                            "Could not join {ssid}: {e}. Check the password and try again."
                        )),
                        422,
                    )
                }
            };
        }
        match self.sta_ip {
            Some(ip) => self.joined_page(ip, provisioned),
            None => self.form(self.reason.clone().as_deref(), 200),
        }
    }

    fn form(&mut self, error: Option<&str>, status: u16) -> Response {
        let options: String = self
            .scan()
            .iter()
            .map(|n| format!("<option>{}</option>", escape(n)))
            .collect();
        let body = format!(
            "<p>Choose your home Wi-Fi network. The board will join it and then \
show you its address.</p>{}\
<form method=\"post\" action=\"/setup/wifi\">\
<label for=\"ssid\">Network</label><select id=\"ssid\" name=\"ssid\">{options}</select>\
<label for=\"ssid_other\">Or type its name</label>\
<input type=\"text\" id=\"ssid_other\" name=\"ssid_other\" autocapitalize=\"none\" autocorrect=\"off\">\
<label for=\"password\">Wi-Fi password</label>\
<input type=\"password\" id=\"password\" name=\"password\" autocomplete=\"off\">\
<p class=\"hint\">Joining takes up to 20 seconds. Stay on the {AP_SSID} network.</p>\
<div class=\"buttons\"><button class=\"approve\">Join network</button></div></form>",
            html::error(error)
        );
        html::page(status, "Connect to Wi-Fi", &body)
    }

    fn joined_page(&self, ip: Ipv4Addr, provisioned: bool) -> Response {
        let next = if provisioned {
            "<div class=\"buttons\"><a class=\"button\" href=\"/\">Continue</a></div>"
        } else {
            "<p>Next, create your household. Stay on this network until you have finished.</p>\
<div class=\"buttons\"><a class=\"button\" href=\"/setup\">Create your household</a></div>"
        };
        let body = format!(
            "<p class=\"ok\">Connected to <strong>{ssid}</strong>.</p>\
<p>On your home network, this board is at <code>http://{ip}/</code> \
(also <code>http://mastomini.local/</code> on most computers and iPhones). \
Write the address down or bookmark it.</p>{next}\
<p class=\"hint\">The {AP_SSID} network closes a couple of minutes after your household is created. \
Then reconnect your phone to {ssid}.</p>",
            ssid = escape(&self.ssid)
        );
        html::page(200, "Wi-Fi connected", &body)
    }
}

/// Answer every DNS `A` query with the setup address. Runs forever.
pub fn dns_responder() {
    let socket = match UdpSocket::bind("0.0.0.0:53") {
        Ok(s) => s,
        Err(e) => {
            log::warn!("Captive DNS unavailable: {e}");
            return;
        }
    };
    let mut buf = [0u8; 512];
    loop {
        let Ok((len, peer)) = socket.recv_from(&mut buf) else {
            continue;
        };
        if let Some(reply) = dns_reply(&buf[..len]) {
            let _ = socket.send_to(&reply, peer);
        }
    }
}
