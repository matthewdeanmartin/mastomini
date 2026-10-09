//! The setup network's pages (spec/06). miniframework runs the radio: it
//! joins the network saved in `mm_wifi`, or opens the open network
//! `mastomini-setup` with a DNS responder that points every name at the
//! board, so phones show these pages as a captive portal. The page lists
//! nearby networks; the board joins the chosen one while keeping the setup
//! network up, shows its new address, and saves the credentials in the
//! system NVS partition. The setup network closes shortly after the
//! household account has been created.

use mastomini::api::html;
use mastomini::http::{Params, Request, Response};
use mastomini::server::Setup;
use mastomini::text::escape;
use miniframework::esp::WifiSetup;
use std::net::Ipv4Addr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const AP_SSID: &str = "mastomini-setup";
/// Saved Wi-Fi credentials, in the system `nvs` partition (not `store`).
pub const NAMESPACE: &str = "mm_wifi";
/// How long the setup network stays up once the household exists.
const CLOSE_AFTER_PROVISIONED: Duration = Duration::from_secs(120);

pub struct BoardSetup {
    wifi: WifiSetup,
    provisioned_at: Mutex<Option<Instant>>,
}

impl BoardSetup {
    pub fn new(wifi: WifiSetup) -> Self {
        Self {
            wifi,
            provisioned_at: Mutex::new(None),
        }
    }

    /// Close the setup network a little after the household is created
    /// (the framework closes it 20 minutes after joining in any case).
    pub fn maybe_close(&self, provisioned: bool) {
        if !self.wifi.active() || self.wifi.joined_for().is_none() || !provisioned {
            return;
        }
        let mut at = self
            .provisioned_at
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let since = *at.get_or_insert_with(Instant::now);
        if since.elapsed() > CLOSE_AFTER_PROVISIONED {
            if let Err(e) = self.wifi.close() {
                log::warn!("Could not close the setup network: {e}");
            }
        }
    }

    fn form(&self, error: Option<&str>, status: u16) -> Response {
        let options: String = self
            .wifi
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
            ssid = escape(&self.wifi.ssid())
        );
        html::page(200, "Wi-Fi connected", &body)
    }
}

impl Setup for BoardSetup {
    fn active(&self) -> bool {
        self.wifi.active()
    }

    fn joined(&self) -> bool {
        self.wifi.station_ip().is_some()
    }

    /// `GET`/`POST /setup/wifi`.
    fn page(&self, req: &Request, provisioned: bool) -> Response {
        if !self.wifi.active() {
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
            return match self.wifi.join(&ssid, &password) {
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
        match self.wifi.station_ip() {
            Some(ip) => self.joined_page(ip, provisioned),
            None => self.form(self.wifi.reason().as_deref(), 200),
        }
    }
}
