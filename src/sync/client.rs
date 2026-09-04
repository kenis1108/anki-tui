use anyhow::{anyhow, bail, Context, Result};
use rand::RngExt;
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

const SYNC_VER: u8 = 11;
const CLIENT_VER: &str = "anki,25.09 (anki-tui),rust";
const HEADER_NAME: &str = "anki-sync";
const ORIGINAL_SIZE: &str = "anki-original-size";

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct SyncMeta {
    #[serde(default)]
    pub mod_: i64,
    #[serde(default, rename = "mod")]
    pub modification: i64,
    #[serde(default)]
    pub scm: i64,
    #[serde(default)]
    pub usn: i32,
    #[serde(default)]
    pub msg: String,
    #[serde(default = "default_true")]
    pub cont: bool,
    #[serde(default)]
    pub empty: bool,
}

fn default_true() -> bool {
    true
}

impl SyncMeta {
    pub fn mod_time(&self) -> i64 {
        if self.modification != 0 {
            self.modification
        } else {
            self.mod_
        }
    }
}

pub struct AnkiWebClient {
    endpoint: String,
    hkey: String,
    skey: String,
    http: Client,
}

impl AnkiWebClient {
    pub const DEFAULT_ENDPOINT: &'static str = "https://sync.ankiweb.net/";

    pub fn new(endpoint: &str, hkey: &str) -> Self {
        let mut endpoint = endpoint.to_string();
        if !endpoint.ends_with('/') {
            endpoint.push('/');
        }
        let http = Client::builder()
            .timeout(Duration::from_secs(120))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("build http client");
        Self {
            endpoint,
            hkey: hkey.to_string(),
            skey: random_skey(),
            http,
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn hkey(&self) -> &str {
        &self.hkey
    }

    pub fn login(&mut self, user: &str, pass: &str) -> Result<()> {
        #[derive(Deserialize)]
        struct HostKey {
            key: String,
        }
        let body = json!({ "u": user, "p": pass });
        let raw = self.request(
            "sync",
            "hostKey",
            Some(body.to_string().into_bytes()),
            None,
        )?;
        let parsed: HostKey = serde_json::from_slice(&raw).context("parse hostKey response")?;
        self.hkey = parsed.key;
        Ok(())
    }

    pub fn meta(&mut self) -> Result<SyncMeta> {
        let body = json!({ "v": SYNC_VER, "cv": CLIENT_VER });
        let raw = self.request(
            "sync",
            "meta",
            Some(body.to_string().into_bytes()),
            None,
        )?;
        let meta: SyncMeta = serde_json::from_slice(&raw).context("parse meta response")?;
        Ok(meta)
    }

    pub fn download(&mut self) -> Result<Vec<u8>> {
        self.request("sync", "download", Some(b"{}".to_vec()), None)
    }

    pub fn upload(&mut self, collection: &[u8]) -> Result<()> {
        let raw = self.request("sync", "upload", None, Some(collection))?;
        if raw == b"OK" || raw == b"\"OK\"" {
            Ok(())
        } else {
            let msg = String::from_utf8_lossy(&raw);
            bail!("upload rejected: {msg}")
        }
    }

    pub fn meta_raw(&mut self) -> Result<Vec<u8>> {
        let body = json!({ "v": SYNC_VER, "cv": CLIENT_VER });
        self.request("sync", "meta", Some(body.to_string().into_bytes()), None)
    }

    pub fn json_method(&mut self, method: &str, body: serde_json::Value) -> Result<serde_json::Value> {
        let raw = self.request(
            "sync",
            method,
            Some(body.to_string().into_bytes()),
            None,
        )?;
        serde_json::from_slice(&raw).with_context(|| format!("parse {method} JSON"))
    }

    pub fn request_json_or_int(
        &mut self,
        method: &str,
        body: serde_json::Value,
    ) -> Result<crate::sync::delta::FinishParse> {
        let raw = self.request(
            "sync",
            method,
            Some(body.to_string().into_bytes()),
            None,
        )?;
        if let Ok(n) = serde_json::from_slice::<i64>(&raw) {
            return Ok(crate::sync::delta::FinishParse::Int(n));
        }
        let v: serde_json::Value = serde_json::from_slice(&raw)?;
        Ok(crate::sync::delta::FinishParse::Json(v))
    }

    /// Media sync endpoints under `/msync/`; responses are `{data, err}` wrappers.
    pub fn msync_json<T: serde::de::DeserializeOwned>(
        &mut self,
        method: &str,
        body: serde_json::Value,
    ) -> Result<T> {
        let raw = self.request(
            "msync",
            method,
            Some(body.to_string().into_bytes()),
            None,
        )?;
        #[derive(serde::Deserialize)]
        struct Wrap<T> {
            data: Option<T>,
            #[serde(default)]
            err: String,
        }
        // untagged: sometimes just the value
        if let Ok(w) = serde_json::from_slice::<Wrap<T>>(&raw) {
            if !w.err.is_empty() {
                bail!("msync {method}: {}", w.err);
            }
            return w
                .data
                .ok_or_else(|| anyhow!("msync {method}: missing data"));
        }
        serde_json::from_slice(&raw).with_context(|| format!("parse msync {method}"))
    }

    pub fn msync_bytes(&mut self, method: &str, body: serde_json::Value) -> Result<Vec<u8>> {
        self.request(
            "msync",
            method,
            Some(body.to_string().into_bytes()),
            None,
        )
    }

    pub fn msync_upload_zip(&mut self, zip: &[u8]) -> Result<serde_json::Value> {
        let raw = self.request("msync", "uploadChanges", None, Some(zip))?;
        #[derive(serde::Deserialize)]
        struct Wrap {
            data: Option<serde_json::Value>,
            #[serde(default)]
            err: String,
        }
        if let Ok(w) = serde_json::from_slice::<Wrap>(&raw) {
            if !w.err.is_empty() {
                bail!("msync uploadChanges: {}", w.err);
            }
            return w
                .data
                .ok_or_else(|| anyhow!("msync uploadChanges: missing data"));
        }
        serde_json::from_slice(&raw).context("parse uploadChanges")
    }

    fn request(
        &mut self,
        prefix: &str,
        method: &str,
        json_body: Option<Vec<u8>>,
        raw_body: Option<&[u8]>,
    ) -> Result<Vec<u8>> {
        let uncompressed = match (json_body, raw_body) {
            (_, Some(raw)) => raw.to_vec(),
            (Some(json), None) => json,
            (None, None) => b"{}".to_vec(),
        };
        let compressed =
            zstd::encode_all(uncompressed.as_slice(), 0).context("zstd compress request")?;

        let header = json!({
            "v": SYNC_VER,
            "k": self.hkey,
            "c": CLIENT_VER,
            "s": self.skey,
        })
        .to_string();

        let url = format!("{}{prefix}/{method}", self.endpoint);
        let mut headers = HeaderMap::new();
        headers.insert(
            HEADER_NAME,
            HeaderValue::from_str(&header).context("anki-sync header")?,
        );
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        );
        headers.insert(
            ORIGINAL_SIZE,
            HeaderValue::from_str(&uncompressed.len().to_string())?,
        );

        let resp = self
            .http
            .post(&url)
            .headers(headers)
            .body(compressed)
            .send()
            .with_context(|| format!("POST {url}"))?;

        if resp.status() == StatusCode::PERMANENT_REDIRECT {
            let loc = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| anyhow!("308 without Location"))?
                .to_string();
            let mut ep = loc;
            if !ep.ends_with('/') {
                ep.push('/');
            }
            self.endpoint = ep;
            return self.request(prefix, method, None, Some(uncompressed.as_slice()));
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            bail!("sync {prefix}/{method} failed ({status}): {body}");
        }

        let has_size = resp.headers().contains_key(ORIGINAL_SIZE);
        let bytes = resp.bytes().context("read response body")?.to_vec();
        if has_size && !bytes.is_empty() {
            zstd::decode_all(bytes.as_slice()).context("zstd decompress response")
        } else if has_size && bytes.is_empty() {
            Ok(Vec::new())
        } else {
            Ok(bytes)
        }
    }
}

fn random_skey() -> String {
    const TABLE: &[u8] =
        b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::rng();
    (0..8)
        .map(|_| TABLE[rng.random_range(0..TABLE.len())] as char)
        .collect()
}
