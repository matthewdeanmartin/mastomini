use crate::{Cloud, Error, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bank {
    pub id: String,
    pub secret: String,
    pub origin: String,
    pub audience: String,
}
impl Bank {
    pub fn validate(&self) -> Result<()> {
        if !crate::name(&self.id)
            || self.secret.len() < 32
            || self.secret.len() > 128
            || !origin_valid(&self.origin)
            || !origin_valid(&self.audience)
        {
            return Err(Error::new(
                400,
                "bank requires safe id, 32–128 byte secret and origin/audience URLs without a path",
            ));
        }
        Ok(())
    }
}
pub fn origin_valid(s: &str) -> bool {
    s.strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .is_some_and(|host| {
            !host.is_empty()
                && host.len() <= 160
                && host
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b".-:".contains(&b))
        })
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Claims {
    pub bank: String,
    pub username: String,
    pub aud: String,
    pub iat: u64,
    pub exp: u64,
}
pub fn identity_valid(username: &str) -> bool {
    !username.trim().is_empty()
        && username.len() <= 64
        && !username.contains('@')
        && !username.chars().any(char::is_control)
}
pub fn authenticate(cloud: &Cloud, token: &str) -> Result<String> {
    let unauthorized = || Error::new(401, "invalid or expired bank token");
    if token.len() > 1024 {
        return Err(unauthorized());
    }
    let (payload, signature) = token
        .strip_prefix("mc1.")
        .and_then(|s| s.split_once('.'))
        .ok_or_else(unauthorized)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| unauthorized())?;
    let claims: Claims = serde_json::from_slice(&bytes).map_err(|_| unauthorized())?;
    let bank = cloud
        .state
        .banks
        .iter()
        .find(|b| b.id == claims.bank)
        .ok_or_else(unauthorized)?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| unauthorized())?;
    let mut mac =
        Hmac::<Sha256>::new_from_slice(bank.secret.as_bytes()).map_err(|_| unauthorized())?;
    mac.update(format!("mc1.{payload}").as_bytes());
    mac.verify_slice(&signature).map_err(|_| unauthorized())?;
    let now = cloud.now();
    if now < 1_700_000_000 {
        return Err(Error::new(503, "clock not synchronized"));
    }
    if claims.aud != bank.audience
        || !identity_valid(&claims.username)
        || claims.iat > now + 30
        || claims.exp <= now
        || claims.exp <= claims.iat
        || claims.exp - claims.iat > 300
    {
        return Err(unauthorized());
    }
    Ok(format!("{}@{}", claims.username, claims.bank))
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Copyright {
    pub bucket: String,
    pub key: String,
    pub hash: String,
    pub owner: String,
    pub title: String,
    pub license: String,
    pub revision: u64,
}
impl Copyright {
    pub fn validate(&self, cloud: &Cloud) -> Result<()> {
        self.validate_blobs(&cloud.state.blobs)
    }
    pub fn validate_blobs(&self, blobs: &[crate::store::Blob]) -> Result<()> {
        crate::store::object_name(&self.bucket, &self.key)?;
        let (user, bank) = self
            .owner
            .rsplit_once('@')
            .ok_or_else(|| Error::new(400, "owner must be username@bank"))?;
        if !identity_valid(user)
            || !crate::name(bank)
            || self.title.trim().is_empty()
            || self.title.len() > 80
            || self.license.trim().is_empty()
            || self.license.len() > 256
        {
            return Err(Error::new(
                400,
                "invalid owner, title (1–80 bytes) or license (1–256 bytes)",
            ));
        }
        let blob = blobs
            .iter()
            .find(|b| b.bucket == self.bucket && b.key == self.key)
            .ok_or_else(|| Error::new(404, "stored file not found"))?;
        if blob.hash != self.hash {
            return Err(Error::new(409, "file hash differs from registry request"));
        }
        Ok(())
    }
}
pub fn query(path: &str) -> String {
    path.split_once('?')
        .map(|(_, q)| {
            q.split('&')
                .find_map(|p| p.strip_prefix("q="))
                .map(|v| {
                    percent_encoding::percent_decode_str(v)
                        .decode_utf8_lossy()
                        .to_lowercase()
                })
                .unwrap_or_default()
        })
        .unwrap_or_default()
}
