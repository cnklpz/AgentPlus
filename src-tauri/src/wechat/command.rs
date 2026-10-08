//! What a message sent to the bot asks for. Everything that isn't a command is text for a
//! session. Full-width `／` and `＃` (typed with a Chinese input method) count as `/` and `#`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Accept,
    /// Accept, and don't ask again for the rest of the session.
    Always,
    Decline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    Help,
    /// Recent sessions; how many.
    List(Option<usize>),
    /// Make this session the current one; `force` ignores the "open in Codex" lock.
    Use { no: u32, force: bool },
    /// A new session in this folder (or the default one).
    New(Option<String>),
    /// Interrupt the running turn of this session (or the current one).
    Stop(Option<u32>),
    Status,
    /// `y`, `n`, `a`, optionally with a session number: an answer to an approval, or plain
    /// text when nothing waits for one.
    Approve { no: Option<u32>, decision: Decision },
    /// `#3 text`: send to session 3 without switching.
    To { no: u32, force: bool, text: String },
    Text(String),
    /// An unknown `/command`.
    Unknown(String),
}

fn normalize(s: &str) -> String {
    let s = s.trim();
    match s.chars().next() {
        Some('／') => format!("/{}", &s['／'.len_utf8()..]),
        Some('＃') => format!("#{}", &s['＃'.len_utf8()..]),
        _ => s.to_string(),
    }
}

/// "3", "#3", "3!", "#3!" → (3, force).
fn session_ref(s: &str) -> Option<(u32, bool)> {
    let s = s.trim().trim_start_matches(['#', '＃']);
    let (n, force) = match s.strip_suffix(['!', '！']) {
        Some(n) => (n, true),
        None => (s, false),
    };
    n.parse().ok().filter(|n| *n > 0).map(|n| (n, force))
}

fn approval(s: &str) -> Option<Cmd> {
    let lower = s.to_lowercase();
    let (word, rest) = match lower.find(|c: char| c.is_ascii_digit() || c == '#' || c.is_whitespace()) {
        Some(i) => (&lower[..i], lower[i..].trim()),
        None => (lower.as_str(), ""),
    };
    let decision = match word {
        "y" | "yes" | "同意" | "允许" => Decision::Accept,
        "a" | "always" | "始终" | "总是" => Decision::Always,
        "n" | "no" | "拒绝" | "不" => Decision::Decline,
        _ => return None,
    };
    let no = if rest.is_empty() {
        None
    } else {
        match session_ref(rest) {
            Some((n, false)) => Some(n),
            _ => return None,
        }
    };
    Some(Cmd::Approve { no, decision })
}

pub fn parse(input: &str) -> Cmd {
    let s = normalize(input);
    if let Some(body) = s.strip_prefix('/') {
        let (name, arg) = match body.find(char::is_whitespace) {
            Some(i) => (&body[..i], body[i..].trim()),
            None => (body, ""),
        };
        let arg_opt = (!arg.is_empty()).then(|| arg.to_string());
        return match name.to_lowercase().as_str() {
            "help" | "h" | "帮助" => Cmd::Help,
            "ls" | "list" | "列表" => Cmd::List(arg.parse().ok().filter(|n| *n > 0)),
            "use" | "u" | "切换" => match session_ref(arg) {
                Some((no, force)) => Cmd::Use { no, force },
                None => Cmd::Unknown(s.clone()),
            },
            "new" | "新建" => Cmd::New(arg_opt),
            "stop" | "停止" => match arg {
                "" => Cmd::Stop(None),
                a => match session_ref(a) {
                    Some((n, _)) => Cmd::Stop(Some(n)),
                    None => Cmd::Unknown(s.clone()),
                },
            },
            "status" | "st" | "状态" => Cmd::Status,
            _ => Cmd::Unknown(s.clone()),
        };
    }
    if s.starts_with('#') {
        let end = s.find(char::is_whitespace).unwrap_or(s.len());
        if let Some((no, force)) = session_ref(&s[..end]) {
            let text = s[end..].trim().to_string();
            if !text.is_empty() {
                return Cmd::To { no, force, text };
            }
        }
    }
    if let Some(c) = approval(&s) {
        return c;
    }
    Cmd::Text(input.trim().to_string())
}

/// The session a quoted bot message came from: its tag `【#3 …】` at the start.
pub fn tag_in(quoted: &str) -> Option<u32> {
    let rest = quoted.trim_start().strip_prefix("【#")?;
    let end = rest.find(|c: char| !c.is_ascii_digit())?;
    rest[..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands() {
        assert_eq!(parse("/help"), Cmd::Help);
        assert_eq!(parse("/ls"), Cmd::List(None));
        assert_eq!(parse("/ls 20"), Cmd::List(Some(20)));
        assert_eq!(parse("/ls x"), Cmd::List(None));
        assert_eq!(parse("/use 3"), Cmd::Use { no: 3, force: false });
        assert_eq!(parse("/use #3!"), Cmd::Use { no: 3, force: true });
        assert_eq!(parse("／use 2"), Cmd::Use { no: 2, force: false });
        assert_eq!(parse("/use"), Cmd::Unknown("/use".into()));
        assert_eq!(parse("/use 0"), Cmd::Unknown("/use 0".into()));
        assert_eq!(parse("/new"), Cmd::New(None));
        assert_eq!(parse("/new C:\\code\\blog"), Cmd::New(Some("C:\\code\\blog".into())));
        assert_eq!(parse("/stop"), Cmd::Stop(None));
        assert_eq!(parse("/stop 2"), Cmd::Stop(Some(2)));
        assert_eq!(parse("/STATUS"), Cmd::Status);
        assert_eq!(parse("/foo"), Cmd::Unknown("/foo".into()));
    }

    #[test]
    fn parses_session_targets() {
        assert_eq!(parse("#3 run the tests"), Cmd::To { no: 3, force: false, text: "run the tests".into() });
        assert_eq!(parse("＃3! 跑一下"), Cmd::To { no: 3, force: true, text: "跑一下".into() });
        // A heading-like message or a bare tag is plain text.
        assert_eq!(parse("#3"), Cmd::Text("#3".into()));
        assert_eq!(parse("# title"), Cmd::Text("# title".into()));
        assert_eq!(parse("#abc def"), Cmd::Text("#abc def".into()));
    }

    #[test]
    fn parses_approvals() {
        assert_eq!(parse("y"), Cmd::Approve { no: None, decision: Decision::Accept });
        assert_eq!(parse("Y2"), Cmd::Approve { no: Some(2), decision: Decision::Accept });
        assert_eq!(parse("n #2"), Cmd::Approve { no: Some(2), decision: Decision::Decline });
        assert_eq!(parse("a3"), Cmd::Approve { no: Some(3), decision: Decision::Always });
        assert_eq!(parse("同意"), Cmd::Approve { no: None, decision: Decision::Accept });
        assert_eq!(parse("拒绝 1"), Cmd::Approve { no: Some(1), decision: Decision::Decline });
        assert_eq!(parse("yes please"), Cmd::Text("yes please".into()));
        assert_eq!(parse("no2x"), Cmd::Text("no2x".into()));
        assert_eq!(parse("y2!"), Cmd::Text("y2!".into()));
        assert_eq!(parse("hello"), Cmd::Text("hello".into()));
    }

    #[test]
    fn reads_tags() {
        assert_eq!(tag_in("【#12 AgentPlus】\nDone"), Some(12));
        assert_eq!(tag_in("  【#3】"), Some(3));
        assert_eq!(tag_in("#3 hi"), None);
        assert_eq!(tag_in("【#】"), None);
    }
}
