//! Mojang lookups: username → account UUID, and a player's face cropped
//! from their skin. Talks to Mojang directly (no third-party skin
//! services). Callers cache results; Mojang rate-limits these endpoints.

use std::time::Duration;

use base64::Engine;

use crate::profile::{Profile, normalize_uuid, valid_name};

/// Lookup failures.
#[derive(Debug, thiserror::Error)]
pub enum MojangError {
    /// Mojang asked us to slow down.
    #[error("Mojang is rate limiting lookups; try again in a minute")]
    RateLimited,
    /// Network or server trouble.
    #[error("couldn't reach Mojang: {0}")]
    Unavailable(String),
    /// A reply didn't make sense.
    #[error("unexpected reply from Mojang: {0}")]
    Invalid(String),
}

type Result<T, E = MojangError> = std::result::Result<T, E>;

/// Largest skin image accepted.
const MAX_SKIN_BYTES: usize = 512 * 1024;

/// Mojang API client.
#[derive(Clone)]
pub struct Mojang {
    http: reqwest::Client,
    api: String,
    sessions: String,
}

impl Mojang {
    /// A client for the real Mojang endpoints.
    pub fn new() -> Self {
        Self::with_endpoints("https://api.mojang.com", "https://sessionserver.mojang.com")
    }

    /// A client for other endpoints (tests).
    pub fn with_endpoints(api: &str, sessions: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent(concat!("NetherNode/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();
        Self {
            http,
            api: api.trim_end_matches('/').to_owned(),
            sessions: sessions.trim_end_matches('/').to_owned(),
        }
    }

    /// The account with this username, or `None` if there isn't one.
    pub async fn lookup(&self, name: &str) -> Result<Option<Profile>> {
        if !valid_name(name) {
            return Ok(None);
        }
        let response = self
            .http
            .get(format!("{}/users/profiles/minecraft/{name}", self.api))
            .send()
            .await
            .map_err(|e| MojangError::Unavailable(e.to_string()))?;
        match response.status().as_u16() {
            200 => {
                let body = response
                    .bytes()
                    .await
                    .map_err(|e| MojangError::Unavailable(e.to_string()))?;
                parse_profile(&body).map(Some)
            }
            204 | 404 => Ok(None),
            429 => Err(MojangError::RateLimited),
            other => Err(MojangError::Unavailable(format!("status {other}"))),
        }
    }

    /// The URL of the player's skin image, if they have a custom one.
    pub async fn skin_url(&self, uuid: &str) -> Result<Option<String>> {
        let id = normalize_uuid(uuid)
            .ok_or_else(|| MojangError::Invalid("not a UUID".into()))?
            .replace('-', "");
        let response = self
            .http
            .get(format!("{}/session/minecraft/profile/{id}", self.sessions))
            .send()
            .await
            .map_err(|e| MojangError::Unavailable(e.to_string()))?;
        match response.status().as_u16() {
            200 => {
                let body = response
                    .bytes()
                    .await
                    .map_err(|e| MojangError::Unavailable(e.to_string()))?;
                parse_skin_url(&body)
            }
            204 | 404 => Ok(None),
            429 => Err(MojangError::RateLimited),
            other => Err(MojangError::Unavailable(format!("status {other}"))),
        }
    }

    /// Downloads a skin image (only from Mojang's texture host).
    pub async fn skin(&self, url: &str) -> Result<Vec<u8>> {
        let allowed = url.starts_with("http://textures.minecraft.net/texture/")
            || url.starts_with("https://textures.minecraft.net/texture/");
        if !allowed {
            return Err(MojangError::Invalid("unexpected skin host".into()));
        }
        let url = url.replacen("http://", "https://", 1);
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| MojangError::Unavailable(e.to_string()))?;
        if !response.status().is_success() {
            return Err(MojangError::Unavailable(format!(
                "status {}",
                response.status()
            )));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| MojangError::Unavailable(e.to_string()))?;
        if bytes.len() > MAX_SKIN_BYTES {
            return Err(MojangError::Invalid("skin image too large".into()));
        }
        Ok(bytes.to_vec())
    }
}

impl Default for Mojang {
    fn default() -> Self {
        Self::new()
    }
}

/// `{"id":"<32 hex>","name":"Name"}` → [`Profile`].
pub fn parse_profile(body: &[u8]) -> Result<Profile> {
    #[derive(serde::Deserialize)]
    struct Raw {
        id: String,
        name: String,
    }
    let raw: Raw = serde_json::from_slice(body).map_err(|e| MojangError::Invalid(e.to_string()))?;
    let uuid = normalize_uuid(&raw.id).ok_or_else(|| MojangError::Invalid("bad UUID".into()))?;
    Ok(Profile {
        uuid,
        name: raw.name,
    })
}

/// The skin URL from a session-server profile, whose `textures` property
/// is base64-encoded JSON.
pub fn parse_skin_url(body: &[u8]) -> Result<Option<String>> {
    let profile: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| MojangError::Invalid(e.to_string()))?;
    let Some(encoded) = profile["properties"]
        .as_array()
        .and_then(|props| props.iter().find(|p| p["name"] == "textures"))
        .and_then(|p| p["value"].as_str())
    else {
        return Ok(None);
    };
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| MojangError::Invalid(e.to_string()))?;
    let textures: serde_json::Value =
        serde_json::from_slice(&decoded).map_err(|e| MojangError::Invalid(e.to_string()))?;
    Ok(textures["textures"]["SKIN"]["url"]
        .as_str()
        .map(str::to_owned))
}

/// Crops the face (with its hat layer) from a skin and scales it to
/// `size`×`size` pixels without smoothing. Returns a PNG.
pub fn render_head(skin_png: &[u8], size: u32) -> Result<Vec<u8>, String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(skin_png));
    decoder.set_transformations(
        png::Transformations::normalize_to_color8() | png::Transformations::ALPHA,
    );
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let info = reader.info();
    let (width, height) = (info.width, info.height);
    // Skins are 64×64 (or legacy 64×32); allow HD skins in multiples.
    if !(64..=1024).contains(&width) || width % 64 != 0 || (height != width && height != width / 2)
    {
        return Err("not a skin image".into());
    }
    let mut buf = vec![0; reader.output_buffer_size().ok_or("image too large")?];
    let frame = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let channels = frame.color_type.samples();
    if frame.bit_depth != png::BitDepth::Eight || !(3..=4).contains(&channels) {
        return Err("unsupported skin format".into());
    }
    let scale = width / 64;
    let pixel = |x: u32, y: u32| -> [u8; 4] {
        let i = ((y * frame.width + x) as usize) * channels;
        let a = if channels == 4 { buf[i + 3] } else { 255 };
        [buf[i], buf[i + 1], buf[i + 2], a]
    };
    // Face at (8,8), hat at (40,8), each 8×8 in skin units.
    let face_px = 8 * scale;
    let mut face = vec![[0u8; 4]; (face_px * face_px) as usize];
    for y in 0..face_px {
        for x in 0..face_px {
            let base = pixel(8 * scale + x, 8 * scale + y);
            let hat = pixel(40 * scale + x, 8 * scale + y);
            let alpha = u32::from(hat[3]);
            let mut out = [0, 0, 0, 255];
            for c in 0..3 {
                out[c] =
                    ((u32::from(hat[c]) * alpha + u32::from(base[c]) * (255 - alpha)) / 255) as u8;
            }
            face[(y * face_px + x) as usize] = out;
        }
    }
    let size = size.clamp(8, 512);
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let sx = x * face_px / size;
            let sy = y * face_px / size;
            out.extend_from_slice(&face[(sy * face_px + sx) as usize]);
        }
    }
    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, size, size);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(&out).map_err(|e| e.to_string())?;
    }
    Ok(png_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles() {
        let p =
            parse_profile(br#"{"id":"069a79f444e94726a5befca90e38aaf5","name":"Notch"}"#).unwrap();
        assert_eq!(p.uuid, "069a79f4-44e9-4726-a5be-fca90e38aaf5");
        assert!(parse_profile(b"{}").is_err());
    }

    #[test]
    fn skin_urls() {
        let textures =
            r#"{"textures":{"SKIN":{"url":"http://textures.minecraft.net/texture/abc"}}}"#;
        let encoded = base64::engine::general_purpose::STANDARD.encode(textures);
        let body = format!(
            r#"{{"id":"x","name":"n","properties":[{{"name":"textures","value":"{encoded}"}}]}}"#
        );
        assert_eq!(
            parse_skin_url(body.as_bytes()).unwrap().as_deref(),
            Some("http://textures.minecraft.net/texture/abc")
        );
        assert_eq!(parse_skin_url(br#"{"properties":[]}"#).unwrap(), None);
    }

    fn skin(face: [u8; 4], hat: [u8; 4]) -> Vec<u8> {
        let mut pixels = vec![0u8; 64 * 64 * 4];
        for y in 8..16 {
            for x in 8..16 {
                pixels[(y * 64 + x) * 4..][..4].copy_from_slice(&face);
            }
            for x in 40..48 {
                pixels[(y * 64 + x) * 4..][..4].copy_from_slice(&hat);
            }
        }
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, 64, 64);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&pixels)
            .unwrap();
        out
    }

    fn first_pixel(png_bytes: &[u8]) -> [u8; 4] {
        let mut reader = png::Decoder::new(std::io::Cursor::new(png_bytes))
            .read_info()
            .unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (64, 64));
        [buf[0], buf[1], buf[2], buf[3]]
    }

    #[test]
    fn heads_composite_the_hat_layer() {
        let bare = render_head(&skin([200, 100, 50, 255], [0, 0, 0, 0]), 64).unwrap();
        assert_eq!(first_pixel(&bare), [200, 100, 50, 255]);
        let hat = render_head(&skin([200, 100, 50, 255], [10, 20, 30, 255]), 64).unwrap();
        assert_eq!(first_pixel(&hat), [10, 20, 30, 255]);
        assert!(render_head(b"not a png", 64).is_err());
    }
}
