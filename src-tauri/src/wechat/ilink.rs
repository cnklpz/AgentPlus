//! WeChat ClawBot's iLink relay: plain HTTP + JSON at `ilinkai.weixin.qq.com`.
//!
//! There is no public documentation; this follows Tencent's own OpenClaw channel
//! (`@tencent-weixin/openclaw-weixin` 2.4.9, `dist/src/api/api.js` and `auth/login-qr.js`):
//! - Sign-in: `get_bot_qrcode` gives a QR code to scan, `get_qrcode_status` long-polls until
//!   the phone confirms and returns the bot token, the account's API base and the user id.
//! - Receive: `getupdates` long-polls (35 s) with an opaque cursor (`get_updates_buf`).
//! - Send: `sendmessage`, echoing the `context_token` of a message received from that user.
//! - Errcode -14 means the bot token is no longer valid (scan again).

use crate::i18n::l;
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

/// Where WeChat keeps message media (`CDN_BASE_URL` of the channel). Downloads carry no token.
const CDN_BASE: &str = "https://novac2c.cdn.weixin.qq.com/c2c";
/// Largest picture downloaded.
pub const IMAGE_MAX: u64 = 20 * 1024 * 1024;
const CDN_TIMEOUT: Duration = Duration::from_secs(30);

/// A picture in a message: where to download it and the key it is encrypted with.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageRef {
    pub url: String,
    /// None: stored unencrypted.
    pub key: Option<[u8; 16]>,
}

/// An AES key as the relay sends it: base64 of the 16 bytes (pictures), or base64 of
/// their 32-character hex form (other media).
fn media_key(b64: &str) -> Option<[u8; 16]> {
    let raw = B64.decode(b64.trim()).ok()?;
    if let Ok(k) = <[u8; 16]>::try_from(raw.as_slice()) {
        return Some(k);
    }
    let hex = std::str::from_utf8(&raw).ok()?;
    hex_key(hex)
}

fn hex_key(hex: &str) -> Option<[u8; 16]> {
    let hex = hex.trim();
    if hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut k = [0u8; 16];
    for (i, b) in k.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(k)
}

/// The picture of an `image_item`, or None when it can't be fetched from a WeChat host.
fn image_ref(img: &Value) -> Option<ImageRef> {
    let media = &img["media"];
    let url = match media["full_url"].as_str().map(str::trim).filter(|u| !u.is_empty()) {
        // Only WeChat's own hosts, as for the relay.
        Some(u) if trusted_base(u) => u.to_string(),
        _ => {
            let q = media["encrypt_query_param"].as_str().map(str::trim).filter(|q| !q.is_empty())?;
            let mut u = url::Url::parse(&format!("{CDN_BASE}/download")).ok()?;
            u.query_pairs_mut().append_pair("encrypted_query_param", q);
            u.to_string()
        }
    };
    // The hex key on the item is preferred, as in the channel.
    let key = img["aeskey"].as_str().and_then(hex_key).or_else(|| media["aes_key"].as_str().and_then(media_key));
    Some(ImageRef { url, key })
}

/// AES-128-ECB with PKCS#7 padding.
fn decrypt_ecb(data: &[u8], key: &[u8; 16]) -> Result<Vec<u8>> {
    use aes::cipher::{BlockDecrypt, KeyInit};
    if data.is_empty() || !data.len().is_multiple_of(16) {
        bail!("{}", tr!("The picture from WeChat is damaged ({n} bytes)", "微信发来的图片已损坏（{n} 字节）", n = data.len()));
    }
    let cipher = aes::Aes128::new(key.into());
    let mut out = data.to_vec();
    for block in out.as_chunks_mut::<16>().0 {
        cipher.decrypt_block(block.into());
    }
    let pad = usize::from(*out.last().expect("not empty"));
    if pad == 0 || pad > 16 || out[out.len() - pad..].iter().any(|b| usize::from(*b) != pad) {
        bail!("{}", l("The picture from WeChat couldn't be decrypted", "微信发来的图片无法解密"));
    }
    out.truncate(out.len() - pad);
    Ok(out)
}

/// The file extension of a picture from its first bytes.
pub fn image_ext(data: &[u8]) -> &'static str {
    match data {
        [0x89, b'P', b'N', b'G', ..] => "png",
        [b'G', b'I', b'F', b'8', ..] => "gif",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "webp",
        [b'B', b'M', ..] => "bmp",
        _ => "jpg",
    }
}

/// Downloads and decrypts a picture.
pub fn download_image(img: &ImageRef) -> Result<Vec<u8>> {
    use std::io::Read;
    if !trusted_base(&img.url) {
        bail!("{}", tr!("Refusing to download a picture from {url}", "拒绝从 {url} 下载图片", url = img.url));
    }
    let http = reqwest::blocking::Client::builder().connect_timeout(Duration::from_secs(10)).redirect(reqwest::redirect::Policy::none()).build()?;
    let resp = http.get(&img.url).timeout(CDN_TIMEOUT).send()?;
    let status = resp.status();
    if !status.is_success() {
        bail!("{}", tr!("Downloading the picture failed: HTTP {code}", "下载图片失败：HTTP {code}", code = status.as_u16()));
    }
    if resp.content_length().is_some_and(|n| n > IMAGE_MAX + 16) {
        bail!("{}", l("The picture is larger than 20 MB", "图片超过 20 MB"));
    }
    let mut data = vec![];
    resp.take(IMAGE_MAX + 17).read_to_end(&mut data)?;
    if data.len() as u64 > IMAGE_MAX + 16 {
        bail!("{}", l("The picture is larger than 20 MB", "图片超过 20 MB"));
    }
    match &img.key {
        Some(k) => decrypt_ecb(&data, k),
        None => Ok(data),
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
    /// The part of the quoted message the user selected, when they quoted only a part.
    pub quote_part: Option<String>,
    /// It carried a file, a video, or a picture that can't be fetched (not passed on).
    pub media: bool,
    /// Pictures to download and pass on.
    pub images: Vec<ImageRef>,
    /// The pictures once downloaded.
    pub pictures: Vec<std::path::PathBuf>,
    /// It carried a voice message WeChat gave no text for.
    pub voice_blank: bool,
}

fn item_text(item: &Value) -> Option<String> {
    match item["type"].as_i64() {
        Some(1) => item["text_item"]["text"].as_str().map(String::from),
        // Voice: the relay's transcription.
        Some(3) => item["voice_item"]["text"].as_str().filter(|s| !s.is_empty()).map(String::from),
        _ => None,
    }
}

/// The start of the `n`th (from 0) `needle` in `text` at or after `from`.
fn nth_index(text: &str, needle: &str, n: u64, from: usize) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    let mut pos = from;
    for i in 0..=n {
        pos += text.get(pos..)?.find(needle)?;
        if i < n {
            pos += needle.len();
        }
    }
    Some(pos)
}

/// The selected part of a quoted message (`ref_msg.partial_text`: where it starts and ends,
/// as text and which occurrence). Like the channel without its MD5 check: the end counted
/// over the whole text first, then from the start. Without the full text, the two ends.
fn partial_quote(full: &str, p: &Value) -> Option<String> {
    let (start, end) = (p["start"].as_str().unwrap_or(""), p["end"].as_str().unwrap_or(""));
    if start.is_empty() || end.is_empty() {
        return None;
    }
    let (si, ei) = (p["startindex"].as_u64().unwrap_or(0), p["endindex"].as_u64().unwrap_or(0));
    let found = nth_index(full, start, si, 0).and_then(|s| {
        [nth_index(full, end, ei, 0), nth_index(full, end, ei, s + start.len())]
            .into_iter()
            .flatten()
            .find(|e| *e >= s)
            .map(|e| full[s..e + end.len()].to_string())
    });
    Some(found.unwrap_or_else(|| if start == end { start.to_string() } else { format!("{start}…{end}") }))
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
    let mut texts: Vec<String> = vec![];
    let (mut media, mut voice_blank) = (false, false);
    let mut images = vec![];
    let (mut quote_id, mut quote_text, mut quote_part) = (None, None, None);
    for item in &items {
        if let Some(t) = item_text(item) {
            texts.push(t);
        } else {
            match item["type"].as_i64() {
                Some(2) => match image_ref(&item["image_item"]) {
                    Some(i) => images.push(i),
                    None => media = true,
                },
                Some(4 | 5) => media = true,
                Some(3) => voice_blank = true,
                _ => {}
            }
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
            if r["partial_text"].is_object() {
                quote_part = partial_quote(&item_text(&r["message_item"]).unwrap_or_default(), &r["partial_text"]);
            }
        }
    }
    Some(Inbound {
        from: m["from_user_id"].as_str()?.to_string(),
        text: texts.join("\n"),
        context_token: m["context_token"].as_str().filter(|s| !s.is_empty()).map(String::from),
        quote_id,
        quote_text,
        quote_part,
        media,
        images,
        pictures: vec![],
        voice_blank,
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
        // Every text item, in order; a voice without text is flagged.
        let two = json!({ "message_type": 1, "from_user_id": "u", "item_list": [
            { "type": 1, "text_item": { "text": "a" } }, { "type": 3, "voice_item": {} }, { "type": 1, "text_item": { "text": "b" } }
        ] });
        let i = parse_message(&two).unwrap();
        assert_eq!(i.text, "a\nb");
        assert!(i.voice_blank && !i.media);
    }

    #[test]
    fn finds_the_selected_part_of_a_quote() {
        let full = "one two three two four";
        let p = |s: &str, si: u64, e: &str, ei: u64| json!({ "start": s, "end": e, "startindex": si, "endindex": ei });
        assert_eq!(partial_quote(full, &p("two", 0, "four", 0)).as_deref(), Some("two three two four"));
        assert_eq!(partial_quote(full, &p("two", 1, "four", 0)).as_deref(), Some("two four"));
        // `endindex` counted from the start when the global count lands before it.
        assert_eq!(partial_quote(full, &p("three", 0, "two", 0)).as_deref(), Some("three two"));
        assert_eq!(partial_quote("一二三四", &p("二", 0, "三", 0)).as_deref(), Some("二三"));
        // Not in the text the relay sent: the two ends.
        assert_eq!(partial_quote("", &p("alpha", 0, "omega", 0)).as_deref(), Some("alpha…omega"));
        assert_eq!(partial_quote(full, &json!({ "start": "", "end": "x" })), None);
        let m = json!({ "message_type": 1, "from_user_id": "u", "item_list": [{ "type": 1, "text_item": { "text": "why?" },
            "ref_msg": { "message_item": { "type": 1, "text_item": { "text": full } }, "partial_text": p("three", 0, "four", 0) } }] });
        assert_eq!(parse_message(&m).unwrap().quote_part.as_deref(), Some("three two four"));
    }

    #[test]
    fn reads_picture_references() {
        let key = [7u8; 16];
        let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(media_key(&B64.encode(key)), Some(key));
        assert_eq!(media_key(&B64.encode(&hex)), Some(key));
        assert_eq!(media_key(&B64.encode([1u8; 8])), None);
        assert_eq!(hex_key(&hex), Some(key));
        assert_eq!(hex_key("zz"), None);

        let m = json!({ "message_type": 1, "from_user_id": "u", "item_list": [
            { "type": 2, "image_item": { "aeskey": hex, "media": { "encrypt_query_param": "a b&c", "aes_key": "ignored" } } },
            { "type": 2, "image_item": { "media": { "full_url": "https://novac2c.cdn.weixin.qq.com/c2c/download?x=1", "aes_key": B64.encode(key) } } },
            // A host that isn't WeChat's: built from the query instead.
            { "type": 2, "image_item": { "media": { "full_url": "https://evil.example/x", "encrypt_query_param": "q" } } },
            { "type": 2, "image_item": { "media": { "full_url": "https://evil.example/x" } } },
        ] });
        let i = parse_message(&m).unwrap();
        assert_eq!(i.images.len(), 3);
        assert_eq!(i.images[0], ImageRef { url: format!("{CDN_BASE}/download?encrypted_query_param=a+b%26c"), key: Some(key) });
        assert_eq!(i.images[1].url, "https://novac2c.cdn.weixin.qq.com/c2c/download?x=1");
        assert_eq!(i.images[2], ImageRef { url: format!("{CDN_BASE}/download?encrypted_query_param=q"), key: None });
        assert!(i.media, "the last one can't be fetched");
        assert!(download_image(&ImageRef { url: "https://evil.example/x".into(), key: None }).is_err());
    }

    #[test]
    fn decrypts_pictures() {
        use aes::cipher::{BlockEncrypt, KeyInit};
        let key = [3u8; 16];
        let plain = b"\x89PNG\r\n\x1a\nhello picture".to_vec();
        let pad = 16 - plain.len() % 16;
        let mut data = plain.clone();
        data.extend(std::iter::repeat_n(pad as u8, pad));
        let cipher = aes::Aes128::new(&key.into());
        for block in data.as_chunks_mut::<16>().0 {
            cipher.encrypt_block(block.into());
        }
        assert_eq!(decrypt_ecb(&data, &key).unwrap(), plain);
        assert!(decrypt_ecb(&data, &[4u8; 16]).is_err(), "wrong key: bad padding");
        assert!(decrypt_ecb(&data[..20], &key).is_err());
        assert_eq!(image_ext(&plain), "png");
        assert_eq!(image_ext(b"RIFF\0\0\0\0WEBPVP8"), "webp");
        assert_eq!(image_ext(&[0xff, 0xd8, 0xff]), "jpg");
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
