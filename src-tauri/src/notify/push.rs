//! Push channels: Bark, ntfy, Gotify, ServerChan and a plain JSON webhook.

use super::{Channels, Message};
use crate::i18n::l;
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::time::Duration;

/// The channels a message goes to: (stable id, display name).
pub const KINDS: [(&str, &str); 5] = [("bark", "Bark"), ("ntfy", "ntfy"), ("gotify", "Gotify"), ("serverchan", "ServerChan"), ("webhook", "Webhook")];

/// AgentPlus's icon, for services that show one per message (Bark on iOS 15+). It must be a
/// public URL: the phone downloads it.
const ICON_URL: &str = "https://raw.githubusercontent.com/cnklpz/AgentPlus/main/src-tauri/icons/128x128@2x.png";

/// Sends one message through one channel.
pub fn send(ch: &Channels, kind: &str, m: &Message) -> Result<()> {
    let client = crate::net::client_with(Duration::from_secs(10)).map_err(|e| anyhow!(e))?;
    let req = match kind {
        "bark" => {
            let (server, key) = bark_target(&ch.bark.server, &ch.bark.key)?;
            let mut body = json!({ "device_key": key, "title": m.title, "body": m.body, "group": "AgentPlus", "icon": ICON_URL });
            if !ch.bark.sound.trim().is_empty() {
                body["sound"] = json!(ch.bark.sound.trim());
            }
            client.post(format!("{server}/push")).header("Content-Type", "application/json; charset=utf-8").body(body.to_string())
        }
        "ntfy" => {
            let topic = ch.ntfy.topic.trim();
            if topic.is_empty() {
                bail!("{}", l("Enter the ntfy topic", "请填写 ntfy 主题"));
            }
            let server = base_url(&ch.ntfy.server, "https://ntfy.sh")?;
            // JSON publishing to the root URL keeps non-ASCII titles intact (headers can't).
            let body = json!({ "topic": topic, "title": m.title, "message": m.body, "tags": ["robot"] });
            let req = client.post(server).header("Content-Type", "application/json; charset=utf-8").body(body.to_string());
            match ch.ntfy.token.trim() {
                "" => req,
                t => req.bearer_auth(t),
            }
        }
        "gotify" => {
            let g = &ch.gotify;
            if g.server.trim().is_empty() {
                bail!("{}", l("Enter the Gotify server address", "请填写 Gotify 服务器地址"));
            }
            let token = g.token.trim();
            if token.is_empty() {
                bail!("{}", l("Enter the Gotify application token", "请填写 Gotify 应用令牌"));
            }
            let server = base_url(&g.server, "")?;
            let body = json!({ "title": m.title, "message": m.body, "priority": gotify_priority(&g.priority)? });
            client.post(format!("{server}/message")).header("X-Gotify-Key", token).header("Content-Type", "application/json; charset=utf-8").body(body.to_string())
        }
        "serverchan" => {
            let url = serverchan_url(&ch.serverchan.key)?;
            client.post(url).form(&[("title", m.title.as_str()), ("desp", m.body.as_str())])
        }
        "webhook" => {
            let url = ch.webhook.url.trim();
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                bail!("{}", l("Enter a webhook URL starting with http:// or https://", "请填写以 http:// 或 https:// 开头的 Webhook 地址"));
            }
            let body = json!({
                "event": "turn_complete",
                "title": m.title,
                "body": m.body,
                "agent": m.agent,
                "project": m.project,
                "durationMs": m.duration_ms,
            });
            client.post(url).header("Content-Type", "application/json; charset=utf-8").body(body.to_string())
        }
        _ => bail!("{}", tr!("Unknown push channel: {kind}", "未知的推送渠道：{kind}")),
    };
    let resp = req.send().map_err(|e| anyhow!(tr!("Request failed: {e}", "请求失败：{e}")))?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        bail!("{}", crate::net::http_error(status, &text));
    }
    check_reply(kind, &text)
}

/// Bark and ServerChan answer HTTP 200 with an error code in the body.
fn check_reply(kind: &str, text: &str) -> Result<()> {
    let Ok(v) = serde_json::from_str::<Value>(text) else { return Ok(()) };
    let code = v["code"].as_i64();
    let ok = match kind {
        "bark" => code.is_none_or(|c| c == 200),
        "serverchan" => code.is_none_or(|c| c == 0),
        _ => true,
    };
    if ok {
        return Ok(());
    }
    let msg = v["message"].as_str().or(v["info"].as_str()).unwrap_or(text);
    bail!("{}", tr!("Rejected by the server: {msg}", "服务器拒绝：{msg}"))
}

/// `server` with no trailing slash, or the default when blank.
fn base_url(server: &str, default: &str) -> Result<String> {
    let s = server.trim().trim_end_matches('/');
    let s = if s.is_empty() { default } else { s };
    if !(s.starts_with("http://") || s.starts_with("https://")) {
        bail!("{}", l("The server address must start with http:// or https://", "服务器地址需以 http:// 或 https:// 开头"));
    }
    Ok(s.to_string())
}

/// The Bark server and device key. The key field also takes the URL the Bark app shows
/// (`https://api.day.app/<key>/…`), whose host then wins over the server field.
pub(crate) fn bark_target(server: &str, key: &str) -> Result<(String, String)> {
    let key = key.trim();
    if key.starts_with("http://") || key.starts_with("https://") {
        let u = url::Url::parse(key).map_err(|_| anyhow!(l("Invalid Bark URL", "Bark 地址无效")))?;
        let k = u.path_segments().and_then(|mut s| s.find(|p| !p.is_empty())).unwrap_or_default();
        if k.is_empty() {
            bail!("{}", l("The Bark URL has no device key", "Bark 地址里没有设备 Key"));
        }
        let port = u.port().map(|p| format!(":{p}")).unwrap_or_default();
        return Ok((format!("{}://{}{port}", u.scheme(), u.host_str().unwrap_or_default()), k.to_string()));
    }
    if key.is_empty() {
        bail!("{}", l("Enter the Bark device key", "请填写 Bark 设备 Key"));
    }
    Ok((base_url(server, "https://api.day.app")?, key.to_string()))
}

/// Gotify's priority field: blank means 5, otherwise a number from 0 to 10.
pub(crate) fn gotify_priority(s: &str) -> Result<u8> {
    match s.trim() {
        "" => Ok(5),
        t => t.parse::<u8>().ok().filter(|p| *p <= 10).ok_or_else(|| anyhow!(l("Gotify priority must be a number from 0 to 10", "Gotify 优先级需为 0 到 10 的数字"))),
    }
}

/// ServerChan Turbo keys (`SCT…`) post to sctapi; ServerChan³ keys (`sctp<uid>t…`) to
/// the user's own push host.
pub(crate) fn serverchan_url(key: &str) -> Result<String> {
    let key = key.trim();
    if key.is_empty() {
        bail!("{}", l("Enter the ServerChan SendKey", "请填写 Server 酱 SendKey"));
    }
    if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        bail!("{}", l("Invalid SendKey", "SendKey 无效"));
    }
    if let Some(rest) = key.strip_prefix("sctp") {
        let uid: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !uid.is_empty() {
            return Ok(format!("https://{uid}.push.ft07.com/send/{key}.send"));
        }
    }
    Ok(format!("https://sctapi.ftqq.com/{key}.send"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bark_key_or_url() {
        assert_eq!(bark_target("", "abc").unwrap(), ("https://api.day.app".into(), "abc".into()));
        assert_eq!(bark_target("https://bark.example.com/", " abc ").unwrap(), ("https://bark.example.com".into(), "abc".into()));
        assert_eq!(bark_target("", "https://api.day.app/KEY123/title").unwrap(), ("https://api.day.app".into(), "KEY123".into()));
        assert_eq!(bark_target("ignored", "http://10.0.0.2:8080/k/").unwrap(), ("http://10.0.0.2:8080".into(), "k".into()));
        assert!(bark_target("", "").is_err());
        assert!(bark_target("", "https://api.day.app/").is_err());
        assert!(bark_target("ftp://x", "k").is_err());
    }

    #[test]
    fn serverchan_hosts() {
        assert_eq!(serverchan_url("SCT123abc").unwrap(), "https://sctapi.ftqq.com/SCT123abc.send");
        assert_eq!(serverchan_url("sctp42tXYZ").unwrap(), "https://42.push.ft07.com/send/sctp42tXYZ.send");
        assert_eq!(serverchan_url("sctpXYZ").unwrap(), "https://sctapi.ftqq.com/sctpXYZ.send");
        assert!(serverchan_url("").is_err());
        assert!(serverchan_url("a/b").is_err());
    }

    #[test]
    fn gotify_priorities() {
        assert_eq!(gotify_priority("").unwrap(), 5);
        assert_eq!(gotify_priority(" 8 ").unwrap(), 8);
        assert_eq!(gotify_priority("0").unwrap(), 0);
        assert!(gotify_priority("11").is_err());
        assert!(gotify_priority("-1").is_err());
        assert!(gotify_priority("high").is_err());
    }

    #[test]
    fn body_codes() {
        assert!(check_reply("bark", r#"{"code":200,"message":"success"}"#).is_ok());
        assert!(check_reply("bark", r#"{"code":400,"message":"failed to get device token"}"#).is_err());
        assert!(check_reply("serverchan", r#"{"code":0}"#).is_ok());
        assert!(check_reply("serverchan", r#"{"code":40001,"info":"bad key"}"#).is_err());
        assert!(check_reply("ntfy", r#"{"code":1}"#).is_ok());
        assert!(check_reply("bark", "not json").is_ok());
    }
}
