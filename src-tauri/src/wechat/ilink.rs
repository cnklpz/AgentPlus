//! WeChat ClawBot's iLink relay: plain HTTP + JSON at `ilinkai.weixin.qq.com`.
//!
//! There is no public documentation; this follows Tencent's own OpenClaw channel
//! (`@tencent-weixin/openclaw-weixin` 2.4.9, `dist/src/api/api.js` and `auth/login-qr.js`):
//! - Sign-in: `get_bot_qrcode` gives a QR code to scan, `get_qrcode_status` long-polls until
//!   the phone confirms and returns the bot token, the account's API base and the user id.
//! - Receive: `getupdates` long-polls (35 s) with an opaque cursor (`get_updates_buf`).
//! - Send: `sendmessage`, echoing the `context_token` of a message received from that user.
//! - Errcode -14 means the bot token is no longer valid (scan again).

use anyhow::{anyhow, bail, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::{json, Value};
use std::time::Duration;

/// Where sign-in starts; an account may be served from another host afterwards.
pub const LOGIN_BASE: &str = "https://ilinkai.weixin.qq.com";
/// The OpenClaw channel release this protocol was taken from. The relay reads it from every
/// request (`base_info.channel_version`, `iLink-App-ClientVersion`).
const CHANNEL_VERSION: &str = "2.4.9";
const APP_ID: &str = "bot";
const BOT_TYPE: &str = "3";
/// Server-side hold of `getupdates`; the relay may ask for another value.
pub const LONG_POLL: Duration = Duration::from_secs(35);
const API_TIMEOUT: Duration = Duration::from_secs(15);
pub const STALE_TOKEN: i64 = -14;
/// Longest text the channel sends in one message (`textChunkLimit`).
pub const TEXT_LIMIT: usize = 4000;

/// `iLink-App-ClientVersion`: 0x00MMNNPP of the channel version.
fn client_version() -> u32 {
    let mut p = CHANNEL_VERSION.split('.').map(|s| s.parse::<u32>().unwrap_or(0) & 0xff);
    let (a, b, c) = (p.next().unwrap_or(0), p.next().unwrap_or(0), p.next().unwrap_or(0));
    (a << 16) | (b << 8) | c
}

/// Whether the bot token may be sent to `base`: only WeChat's own hosts, over HTTPS.
pub fn trusted_base(base: &str) -> bool {
    url::Url::parse(base.trim()).ok().is_some_and(|u| {
        u.scheme() == "https"
            && u.host_str().map(str::to_ascii_lowercase).is_some_and(|h| h == "weixin.qq.com" || h.ends_with(".weixin.qq.com"))
    })
}

fn base_info() -> Value {
    json!({ "channel_version": CHANNEL_VERSION, "bot_agent": format!("AgentPlus/{}", env!("CARGO_PKG_VERSION")) })
}

/// `X-WECHAT-UIN`: a random u32, as decimal text, base64-encoded.
fn random_uin() -> String {
    let mut b = [0u8; 4];
    let _ = getrandom::getrandom(&mut b);
    B64.encode(u32::from_be_bytes(b).to_string())
}

/// A message id the relay sends as a number (uint64) or a string.
pub fn id_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn api_error(v: &Value) -> Option<(i64, String)> {
    let ret = v["ret"].as_i64().unwrap_or(0);
    let code = v["errcode"].as_i64().unwrap_or(0);
    let code = if code != 0 { code } else { ret };
    (code != 0).then(|| (code, v["errmsg"].as_str().unwrap_or("").to_string()))
}

pub struct Client {
    http: reqwest::blocking::Client,
    base: String,
    token: Option<String>,
}

impl Client {
    pub fn new(base: &str, token: Option<String>) -> Result<Client> {
        if token.is_some() && !trusted_base(base) {
            bail!("{}", tr!("Refusing to send the WeChat token to {base}", "拒绝把微信令牌发往 {base}"));
        }
        let http = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Client { http, base: base.trim_end_matches('/').to_string(), token })
    }

    fn url(&self, endpoint: &str) -> String {
        format!("{}/{endpoint}", self.base)
    }

    fn common(&self, req: reqwest::blocking::RequestBuilder) -> reqwest::blocking::RequestBuilder {
        req.header("iLink-App-Id", APP_ID).header("iLink-App-ClientVersion", client_version().to_string())
    }

    fn post(&self, endpoint: &str, body: Value, timeout: Duration) -> Result<Value> {
        let mut req = self
            .common(self.http.post(self.url(endpoint)))
            .timeout(timeout)
            .header("Content-Type", "application/json")
            .header("AuthorizationType", "ilink_bot_token")
            .header("X-WECHAT-UIN", random_uin())
            .body(body.to_string());
        if let Some(t) = &self.token {
            req = req.bearer_auth(t);
        }
        let resp = req.send()?;
        let status = resp.status();
        let text = resp.text()?;
        if !status.is_success() {
            bail!("{endpoint}: HTTP {} {}", status.as_u16(), crate::util::clip(&text, 200));
        }
        serde_json::from_str(&text).map_err(|e| anyhow!("{endpoint}: {e}"))
    }

    /// A new sign-in QR code: (code id, content to show as a QR image).
    pub fn qr_code(&self, previous_tokens: &[String]) -> Result<(String, String)> {
        let v = self.post(&format!("ilink/bot/get_bot_qrcode?bot_type={BOT_TYPE}"), json!({ "local_token_list": previous_tokens }), API_TIMEOUT)?;
        match (v["qrcode"].as_str(), v["qrcode_img_content"].as_str()) {
            (Some(id), Some(content)) => Ok((id.to_string(), content.to_string())),
            _ => bail!("get_bot_qrcode: {}", crate::util::clip(&v.to_string(), 200)),
        }
    }

    /// One long poll of the sign-in status. A timeout reads as "wait".
    pub fn qr_status(&self, qrcode: &str, verify_code: Option<&str>) -> Result<Value> {
        let mut url = url::Url::parse(&self.url("ilink/bot/get_qrcode_status"))?;
        url.query_pairs_mut().append_pair("qrcode", qrcode);
        if let Some(c) = verify_code {
            url.query_pairs_mut().append_pair("verify_code", c);
        }
        match self.common(self.http.get(url)).timeout(LONG_POLL).send() {
            Ok(resp) => {
                let text = resp.text()?;
                serde_json::from_str(&text).map_err(|e| anyhow!("get_qrcode_status: {e}"))
            }
            Err(e) if e.is_timeout() => Ok(json!({ "status": "wait" })),
            Err(e) => Err(e.into()),
        }
    }

    /// One long poll for new messages. Returns the raw response (`msgs`, `get_updates_buf`,
    /// `longpolling_timeout_ms`); a client-side timeout reads as an empty answer.
    pub fn get_updates(&self, cursor: &str, hold: Duration) -> Result<Value> {
        let body = json!({ "get_updates_buf": cursor, "base_info": base_info() });
        match self.post("ilink/bot/getupdates", body, hold + Duration::from_secs(5)) {
            Err(e) if e.downcast_ref::<reqwest::Error>().is_some_and(|e| e.is_timeout()) => Ok(json!({ "ret": 0, "msgs": [] })),
            r => r,
        }
    }

    /// Sends one text message; returns the relay's message id when it gives one.
    pub fn send_text(&self, to: &str, text: &str, context_token: Option<&str>) -> Result<Option<String>> {
        let mut id = [0u8; 8];
        let _ = getrandom::getrandom(&mut id);
        let client_id = format!("agentplus-{}", id.iter().map(|b| format!("{b:02x}")).collect::<String>());
        let body = json!({
            "msg": {
                "from_user_id": "",
                "to_user_id": to,
                "client_id": client_id,
                "message_type": 2,
                "message_state": 2,
                "item_list": [{ "type": 1, "text_item": { "text": text } }],
                "context_token": context_token,
            },
            "base_info": base_info(),
        });
        let v = self.post("ilink/bot/sendmessage", body, API_TIMEOUT)?;
        if let Some((code, msg)) = api_error(&v) {
            bail!("sendmessage: {code} {msg}");
        }
        Ok(id_text(&v["message_id"]))
    }

    /// The ticket `send_typing` needs, per user.
    pub fn typing_ticket(&self, user: &str, context_token: Option<&str>) -> Result<Option<String>> {
        let v = self.post("ilink/bot/getconfig", json!({ "ilink_user_id": user, "context_token": context_token, "base_info": base_info() }), API_TIMEOUT)?;
        Ok(v["typing_ticket"].as_str().filter(|s| !s.is_empty()).map(String::from))
    }

    pub fn send_typing(&self, user: &str, ticket: &str, typing: bool) -> Result<()> {
        let body = json!({ "ilink_user_id": user, "typing_ticket": ticket, "status": if typing { 1 } else { 2 }, "base_info": base_info() });
        self.post("ilink/bot/sendtyping", body, API_TIMEOUT).map(|_| ())
    }

    pub fn notify(&self, start: bool) {
        let ep = if start { "ilink/bot/msg/notifystart" } else { "ilink/bot/msg/notifystop" };
        let _ = self.post(ep, json!({ "base_info": base_info() }), API_TIMEOUT);
    }
}

/// The error of a `getupdates` answer, if it is one.
pub fn updates_error(v: &Value) -> Option<(i64, String)> {
    api_error(v)
}

/// A message from a user, reduced to what the bridge needs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Inbound {
    pub from: String,
    pub text: String,
    pub context_token: Option<String>,
    /// The quoted message's id, when the user replied to one.
    pub quote_id: Option<String>,
    /// The quoted message's text (title and body), when the relay includes it.
    pub quote_text: Option<String>,
    /// It carried an image, file or video (not passed on).
    pub media: bool,
}

fn item_text(item: &Value) -> Option<String> {
    match item["type"].as_i64() {
        Some(1) => item["text_item"]["text"].as_str().map(String::from),
        // Voice: the relay's transcription.
        Some(3) => item["voice_item"]["text"].as_str().filter(|s| !s.is_empty()).map(String::from),
        _ => None,
    }
}

/// A `getupdates` message, or None for anything that isn't a finished direct message from a user.
pub fn parse_message(m: &Value) -> Option<Inbound> {
    if m["message_type"].as_i64() != Some(1) {
        return None;
    }
    if m["group_id"].as_str().is_some_and(|g| !g.is_empty()) {
        return None;
    }
    let items = m["item_list"].as_array().cloned().unwrap_or_default();
    let mut text = String::new();
    let mut media = false;
    let (mut quote_id, mut quote_text) = (None, None);
    for item in &items {
        if let Some(t) = item_text(item) {
            if text.is_empty() {
                text = t;
            }
        } else if matches!(item["type"].as_i64(), Some(2 | 4 | 5)) {
            media = true;
        }
        let r = &item["ref_msg"];
        if r.is_object() {
            quote_id = id_text(&r["svr_id"]).or_else(|| id_text(&r["message_item"]["msg_id"]));
            let parts: Vec<String> = [r["title"].as_str().map(String::from), item_text(&r["message_item"])]
                .into_iter()
                .flatten()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !parts.is_empty() {
                quote_text = Some(parts.join(" | "));
            }
        }
    }
    Some(Inbound {
        from: m["from_user_id"].as_str()?.to_string(),
        text,
        context_token: m["context_token"].as_str().filter(|s| !s.is_empty()).map(String::from),
        quote_id,
        quote_text,
        media,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_version_packs_the_release() {
        assert_eq!(client_version(), (2 << 16) | (4 << 8) | 9);
    }

    #[test]
    fn token_only_goes_to_wechat_hosts() {
        assert!(trusted_base("https://ilinkai.weixin.qq.com"));
        assert!(trusted_base("https://ilinkai-sh.weixin.qq.com/"));
        assert!(!trusted_base("http://ilinkai.weixin.qq.com"));
        assert!(!trusted_base("https://weixin.qq.com.evil.example"));
        assert!(!trusted_base("https://evil.example/weixin.qq.com"));
        assert!(Client::new("https://evil.example", Some("t".into())).is_err());
        assert!(Client::new("https://evil.example", None).is_ok());
    }

    #[test]
    fn parses_text_voice_and_quotes() {
        let m = json!({
            "message_type": 1, "from_user_id": "u@im.wechat", "context_token": "ctx",
            "item_list": [
                { "type": 1, "text_item": { "text": "hi" },
                  "ref_msg": { "svr_id": 123456789012345678u64, "title": "【#2 demo】", "message_item": { "type": 1, "text_item": { "text": "done" } } } }
            ]
        });
        let i = parse_message(&m).unwrap();
        assert_eq!(i.text, "hi");
        assert_eq!(i.from, "u@im.wechat");
        assert_eq!(i.context_token.as_deref(), Some("ctx"));
        assert_eq!(i.quote_id.as_deref(), Some("123456789012345678"));
        assert_eq!(i.quote_text.as_deref(), Some("【#2 demo】 | done"));

        let v = json!({ "message_type": 1, "from_user_id": "u", "item_list": [{ "type": 3, "voice_item": { "text": "语音" } }] });
        assert_eq!(parse_message(&v).unwrap().text, "语音");
        let img = json!({ "message_type": 1, "from_user_id": "u", "item_list": [{ "type": 2 }] });
        let i = parse_message(&img).unwrap();
        assert!(i.media && i.text.is_empty());
    }

    #[test]
    fn skips_bot_and_group_messages() {
        assert!(parse_message(&json!({ "message_type": 2, "from_user_id": "u", "item_list": [] })).is_none());
        assert!(parse_message(&json!({ "message_type": 1, "from_user_id": "u", "group_id": "g", "item_list": [] })).is_none());
        assert!(parse_message(&json!({ "message_type": 1, "item_list": [] })).is_none());
    }

    /// Asks the real relay for a sign-in QR code: `cargo test real_qr_code -- --ignored`.
    #[test]
    #[ignore]
    fn real_qr_code() {
        let (id, content) = Client::new(LOGIN_BASE, None).unwrap().qr_code(&[]).unwrap();
        assert!(!id.is_empty());
        assert!(qrcode::QrCode::new(content.as_bytes()).is_ok(), "{content}");
        let status = Client::new(LOGIN_BASE, None).unwrap().qr_status(&id, None).unwrap();
        assert!(status["status"].is_string(), "{status}");
    }

    #[test]
    fn reads_api_errors() {
        assert_eq!(updates_error(&json!({ "ret": 0, "msgs": [] })), None);
        assert_eq!(updates_error(&json!({ "ret": -14 })), Some((-14, String::new())));
        assert_eq!(updates_error(&json!({ "ret": 1, "errcode": -14, "errmsg": "x" })), Some((-14, "x".into())));
    }
}
