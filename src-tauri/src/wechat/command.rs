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
pub enum ModelCmd {
    /// The session's model, the default and the choices.
    Show,
    /// A model (number in the list, or its name) for the session, from its next turn.
    Set(String),
    /// Codex's default model for new sessions (`config.toml`); None shows it.
    Default(Option<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    Model(ModelCmd),
    /// The session's reasoning effort, from its next turn; None shows the choices, "default"
    /// drops a pending change.
    Effort(Option<String>),
    /// The command list, or the details of one command.
    Help(Option<&'static Spec>),
    /// Recent sessions; how many.
    List(Option<usize>),
    /// Make this session the current one; `force` ignores the "open in Codex" lock.
    Use { no: u32, force: bool },
    /// `/use` alone: which session is current.
    Current,
    /// A new session in this folder (a number picks from `/dirs`), or the default one.
    New(Option<String>),
    /// Recent working folders, numbered for `/new N`.
    Dirs,
    /// Interrupt the running turn of this session (or the current one).
    Stop(Option<u32>),
    /// Interrupt every running turn.
    StopAll,
    Status,
    /// Send the session's last reply again.
    Last(Option<u32>),
    /// Show the session's queued messages, or drop them.
    Queue { no: Option<u32>, clear: bool },
    /// Add text (or what is queued) to the session's running turn now.
    Now { no: Option<u32>, text: Option<String> },
    /// Give the session a title.
    Rename { no: Option<u32>, title: String },
    /// Archive a session in Codex (it leaves `/ls`).
    Archive(Option<u32>),
    /// What changed in the session's folder.
    Diff(Option<u32>),
    /// The session's folder.
    Cwd(Option<u32>),
    /// The session's approval mode; None shows it.
    Mode { no: Option<u32>, mode: Option<String> },
    /// `y`, `n`, `a`, optionally with a session number: an answer to an approval, or plain
    /// text when nothing waits for one.
    Approve { no: Option<u32>, decision: Decision },
    /// `#3 text`: send to session 3 without switching.
    To { no: u32, force: bool, text: String },
    Text(String),
    /// A known command with arguments it doesn't take: its usage.
    Usage(&'static Spec),
    /// An unknown `/command`, and the closest known one.
    Unknown(String, Option<&'static str>),
}

/// A command for `/help`: its names (the first is the main one), its usage line and what it
/// does, as (English, Chinese) pairs.
#[derive(Debug, PartialEq, Eq)]
pub struct Spec {
    pub names: &'static [&'static str],
    pub usage: (&'static str, &'static str),
    pub about: (&'static str, &'static str),
}

pub const SPECS: &[Spec] = &[
    Spec { names: &["help", "h", "帮助"], usage: ("/help [command]", "/help [命令]"), about: ("The command list, or the details of one command.", "命令列表，或某个命令的详细说明。") },
    Spec { names: &["ls", "list", "列表"], usage: ("/ls [count]", "/ls [数量]"), about: ("Recent Codex sessions (10 by default, up to 30). Reply with a number to switch.", "最近的 Codex 会话（默认 10 个，最多 30）。直接回编号即可切换。") },
    Spec { names: &["use", "u", "切换"], usage: ("/use N[!]", "/use 编号[!]"), about: ("Make session N the current one; ! goes ahead even if it may be open in Codex desktop. /use alone shows the current one.", "切换到会话 N；加 ! 表示即使它可能开在 Codex 桌面版里也切换。只发 /use 显示当前会话。") },
    Spec { names: &["new", "n", "新建"], usage: ("/new [folder | N]", "/new [目录 | 编号]"), about: ("Start a session in a folder, folder N of /dirs, or the current session's folder.", "在指定目录、/dirs 的第 N 个目录或当前会话的目录新建会话。") },
    Spec { names: &["dirs", "d", "目录"], usage: ("/dirs", "/dirs"), about: ("Recent working folders, numbered for /new N.", "最近的工作目录，配合 /new 编号 使用。") },
    Spec { names: &["stop", "停止"], usage: ("/stop [N | all]", "/stop [编号 | all]"), about: ("Interrupt the running turn and drop its queue; all stops every session.", "中断正在运行的这一轮并清空排队；all 停止所有会话。") },
    Spec { names: &["status", "st", "状态"], usage: ("/status", "/status"), about: ("The current session, running turns and what waits for you.", "当前会话、正在运行的任务和等你确认的事项。") },
    Spec { names: &["last", "l", "重发"], usage: ("/last [N]", "/last [编号]"), about: ("Send the session's last reply again.", "重发该会话最后一次的回复。") },
    Spec {
        names: &["now", "插话"],
        usage: ("/now [N] [text]", "/now [编号] [消息]"),
        about: (
            "Add a message to the turn that is running instead of queuing it; /now alone sends what is queued. Starts a turn when none runs.",
            "把消息直接插入正在进行的这一轮，不用排队；只发 /now 会把排队的消息立即发过去。没有在运行时就正常开始一轮。",
        ),
    },
    Spec { names: &["queue", "q", "排队"], usage: ("/queue [N] [clear]", "/queue [编号] [clear]"), about: ("The messages waiting for the running turn; clear drops them.", "查看等当前这轮结束后发送的消息；clear 清空。") },
    Spec { names: &["rename", "rn", "改名"], usage: ("/rename [N] title", "/rename [编号] 标题"), about: ("Give the session a title.", "给会话改标题。") },
    Spec { names: &["archive", "归档"], usage: ("/archive N", "/archive 编号"), about: ("Archive session N in Codex; it leaves /ls.", "在 Codex 里归档会话 N，它不再出现在 /ls 里。") },
    Spec { names: &["diff", "改动"], usage: ("/diff [N]", "/diff [编号]"), about: ("What changed in the session's folder (git diff --stat).", "会话目录里的改动（git diff --stat）。") },
    Spec { names: &["cwd", "pwd", "路径"], usage: ("/cwd [N]", "/cwd [编号]"), about: ("The session's folder.", "会话所在的目录。") },
    Spec {
        names: &["mode", "模式"],
        usage: ("/mode [N] [ask | auto | full | readonly | default]", "/mode [编号] [ask | auto | full | readonly | default]"),
        about: (
            "How much the session may do without asking. ask: asks for anything not known safe · auto: works in its folder, asks for more · full: no sandbox, never asks · readonly: reads only · default: stop choosing it from WeChat.",
            "会话无需询问可以做多少事。ask：不确定安全的都问 · auto：在目录内自由工作，超出才问 · full：无沙箱、从不询问 · readonly：只读 · default：不再由微信指定。",
        ),
    },
    Spec { names: &["model", "m", "模型"], usage: ("/model [N | name | default [N]]", "/model [编号 | 名称 | default [编号]]"), about: ("See or switch the session's model; default changes the default for new sessions.", "查看或切换会话的模型；default 修改新会话的默认模型。") },
    Spec { names: &["effort", "e", "推理"], usage: ("/effort [level | default]", "/effort [强度 | default]"), about: ("See or set the reasoning effort; default drops a change not used yet.", "查看或设置推理强度；default 撤销还没生效的修改。") },
];

/// The command with this name or alias (any case).
pub fn spec(name: &str) -> Option<&'static Spec> {
    let name = name.trim().trim_start_matches(['/', '／']).to_lowercase();
    SPECS.iter().find(|s| s.names.contains(&name.as_str()))
}

/// Edit distance in characters.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != *cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

/// The known command closest to a mistyped one: one edit away (fewer than its length, so `/x`
/// doesn't suggest `/h`), or two for names of five letters or more (`/foo` isn't `/now`).
pub fn closest(name: &str) -> Option<&'static str> {
    let lower = name.to_lowercase();
    let name = lower.as_str();
    let n = name.chars().count();
    SPECS
        .iter()
        .flat_map(|s| s.names.iter().map(move |a| (distance(name, a), a.chars().count(), s.names[0])))
        .filter(|(d, len, _)| *d < n.min(*len) && (*d <= 1 || (*d == 2 && n.min(*len) >= 5)))
        .min_by_key(|(d, _, _)| *d)
        .map(|(_, _, main)| main)
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

/// A session number without `!`.
fn session_no(s: &str) -> Option<u32> {
    match session_ref(s) {
        Some((n, false)) => Some(n),
        _ => None,
    }
}

/// `[N]`: nothing, or a session number.
fn opt_session(arg: &str) -> Result<Option<u32>, ()> {
    if arg.is_empty() {
        return Ok(None);
    }
    session_no(arg).map(Some).ok_or(())
}

/// `[N] rest`: a session number when the first word is one, and the rest.
fn split_session(arg: &str) -> (Option<u32>, &str) {
    let (first, rest) = arg.split_once(char::is_whitespace).map_or((arg, ""), |(a, b)| (a, b.trim()));
    match session_no(first) {
        Some(n) => (Some(n), rest),
        None => (None, arg),
    }
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
    let no = if rest.is_empty() { None } else { Some(session_no(rest)?) };
    Some(Cmd::Approve { no, decision })
}

fn command(name: &str, arg: &str) -> Cmd {
    let Some(spec) = spec(name) else {
        return Cmd::Unknown(format!("/{name}"), closest(name));
    };
    let usage = Cmd::Usage(spec);
    let arg_opt = (!arg.is_empty()).then(|| arg.to_string());
    match spec.names[0] {
        "help" => match arg {
            "" => Cmd::Help(None),
            a => self::spec(a).map_or(usage, |s| Cmd::Help(Some(s))),
        },
        "ls" => match arg {
            "" => Cmd::List(None),
            a => a.parse().ok().filter(|n| *n > 0).map_or(usage, |n| Cmd::List(Some(n))),
        },
        "use" => match arg {
            "" => Cmd::Current,
            a => session_ref(a).map_or(usage, |(no, force)| Cmd::Use { no, force }),
        },
        "new" => Cmd::New(arg_opt),
        "dirs" if arg.is_empty() => Cmd::Dirs,
        "stop" => match arg.to_lowercase().as_str() {
            "all" | "全部" => Cmd::StopAll,
            _ => opt_session(arg).map_or(usage, Cmd::Stop),
        },
        "status" if arg.is_empty() => Cmd::Status,
        "last" => opt_session(arg).map_or(usage, Cmd::Last),
        "queue" => {
            let (no, rest) = split_session(arg);
            match rest.to_lowercase().as_str() {
                "" => Cmd::Queue { no, clear: false },
                "clear" | "清空" => Cmd::Queue { no, clear: true },
                _ => usage,
            }
        }
        "now" => match split_session(arg) {
            (no, "") => Cmd::Now { no, text: None },
            (no, text) => Cmd::Now { no, text: Some(text.to_string()) },
        },
        "rename" => match split_session(arg) {
            _ if arg.is_empty() => usage,
            // `/rename 2026`: a title that is only a number.
            (_, "") => Cmd::Rename { no: None, title: arg.to_string() },
            (no, title) => Cmd::Rename { no, title: title.to_string() },
        },
        "archive" => opt_session(arg).map_or(usage, Cmd::Archive),
        "diff" => opt_session(arg).map_or(usage, Cmd::Diff),
        "cwd" => opt_session(arg).map_or(usage, Cmd::Cwd),
        "mode" => {
            let (no, rest) = split_session(arg);
            match rest.to_lowercase().as_str() {
                "" => Cmd::Mode { no, mode: None },
                m @ ("ask" | "auto" | "full" | "readonly" | "default") => Cmd::Mode { no, mode: Some(m.to_string()) },
                "默认" => Cmd::Mode { no, mode: Some("default".into()) },
                _ => usage,
            }
        }
        "model" => Cmd::Model(match arg.split_once(char::is_whitespace).map_or((arg, ""), |(a, b)| (a, b.trim())) {
            ("", _) => ModelCmd::Show,
            ("default" | "默认", rest) => ModelCmd::Default((!rest.is_empty()).then(|| rest.to_string())),
            _ => ModelCmd::Set(arg.to_string()),
        }),
        "effort" => Cmd::Effort(arg_opt.map(|a| if a == "默认" { "default".into() } else { a })),
        _ => usage,
    }
}

pub fn parse(input: &str) -> Cmd {
    let s = normalize(input);
    if let Some(body) = s.strip_prefix('/') {
        let (name, arg) = match body.find(char::is_whitespace) {
            Some(i) => (&body[..i], body[i..].trim()),
            None => (body, ""),
        };
        return command(name, arg);
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

/// `#3 /stop`: the command aimed at session 3, or None when the command doesn't act on one
/// session (it isn't sent as text either).
pub fn for_session(cmd: Cmd, n: u32) -> Option<Cmd> {
    Some(match cmd {
        Cmd::Stop(None) => Cmd::Stop(Some(n)),
        Cmd::Last(None) => Cmd::Last(Some(n)),
        Cmd::Queue { no: None, clear } => Cmd::Queue { no: Some(n), clear },
        Cmd::Now { no: None, text } => Cmd::Now { no: Some(n), text },
        Cmd::Rename { no: None, title } => Cmd::Rename { no: Some(n), title },
        Cmd::Archive(None) => Cmd::Archive(Some(n)),
        Cmd::Diff(None) => Cmd::Diff(Some(n)),
        Cmd::Cwd(None) => Cmd::Cwd(Some(n)),
        Cmd::Mode { no: None, mode } => Cmd::Mode { no: Some(n), mode },
        Cmd::Current => Cmd::Use { no: n, force: false },
        c @ (Cmd::Model(_) | Cmd::Effort(_) | Cmd::Usage(_) | Cmd::Unknown(..)) => c,
        _ => return None,
    })
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

    fn usage(name: &str) -> Cmd {
        Cmd::Usage(spec(name).unwrap())
    }

    #[test]
    fn parses_commands() {
        assert_eq!(parse("/help"), Cmd::Help(None));
        assert_eq!(parse("/help stop"), Cmd::Help(Some(spec("stop").unwrap())));
        assert_eq!(parse("/help /停止"), Cmd::Help(Some(spec("stop").unwrap())));
        assert_eq!(parse("/help nope"), usage("help"));
        assert_eq!(parse("/ls"), Cmd::List(None));
        assert_eq!(parse("/ls 20"), Cmd::List(Some(20)));
        assert_eq!(parse("/ls x"), usage("ls"));
        assert_eq!(parse("/use 3"), Cmd::Use { no: 3, force: false });
        assert_eq!(parse("/use #3!"), Cmd::Use { no: 3, force: true });
        assert_eq!(parse("／use 2"), Cmd::Use { no: 2, force: false });
        assert_eq!(parse("/use"), Cmd::Current);
        assert_eq!(parse("/use 0"), usage("use"));
        assert_eq!(parse("/new"), Cmd::New(None));
        assert_eq!(parse("/n"), Cmd::New(None));
        assert_eq!(parse("/new C:\\code\\blog"), Cmd::New(Some("C:\\code\\blog".into())));
        assert_eq!(parse("/dirs"), Cmd::Dirs);
        assert_eq!(parse("/目录"), Cmd::Dirs);
        assert_eq!(parse("/dirs 2"), usage("dirs"));
        assert_eq!(parse("/new 2"), Cmd::New(Some("2".into())));
        assert_eq!(parse("/stop"), Cmd::Stop(None));
        assert_eq!(parse("/stop 2"), Cmd::Stop(Some(2)));
        assert_eq!(parse("/stop ALL"), Cmd::StopAll);
        assert_eq!(parse("/stop x"), usage("stop"));
        assert_eq!(parse("/STATUS"), Cmd::Status);
        assert_eq!(parse("/foo"), Cmd::Unknown("/foo".into(), None));
    }

    #[test]
    fn parses_new_commands() {
        assert_eq!(parse("/last"), Cmd::Last(None));
        assert_eq!(parse("/last #2"), Cmd::Last(Some(2)));
        assert_eq!(parse("/last x"), usage("last"));
        assert_eq!(parse("/queue"), Cmd::Queue { no: None, clear: false });
        assert_eq!(parse("/q 2 clear"), Cmd::Queue { no: Some(2), clear: true });
        assert_eq!(parse("/排队 清空"), Cmd::Queue { no: None, clear: true });
        assert_eq!(parse("/queue 2 x"), usage("queue"));
        assert_eq!(parse("/now"), Cmd::Now { no: None, text: None });
        assert_eq!(parse("/now also check the logs"), Cmd::Now { no: None, text: Some("also check the logs".into()) });
        assert_eq!(parse("/插话 3 先别提交"), Cmd::Now { no: Some(3), text: Some("先别提交".into()) });
        assert_eq!(parse("/now 3"), Cmd::Now { no: Some(3), text: None });
        assert_eq!(for_session(parse("/now stop here"), 5), Some(Cmd::Now { no: Some(5), text: Some("stop here".into()) }));
        assert_eq!(parse("/rename 3 Fix the updater"), Cmd::Rename { no: Some(3), title: "Fix the updater".into() });
        assert_eq!(parse("/rename Fix it"), Cmd::Rename { no: None, title: "Fix it".into() });
        // A title that is just a number names the current session.
        assert_eq!(parse("/rename 2026"), Cmd::Rename { no: None, title: "2026".into() });
        assert_eq!(parse("/rename"), usage("rename"));
        assert_eq!(parse("/archive 4"), Cmd::Archive(Some(4)));
        assert_eq!(parse("/archive"), Cmd::Archive(None));
        assert_eq!(parse("/diff"), Cmd::Diff(None));
        assert_eq!(parse("/pwd 2"), Cmd::Cwd(Some(2)));
        assert_eq!(parse("/mode"), Cmd::Mode { no: None, mode: None });
        assert_eq!(parse("/mode 2 ReadOnly"), Cmd::Mode { no: Some(2), mode: Some("readonly".into()) });
        assert_eq!(parse("/mode full"), Cmd::Mode { no: None, mode: Some("full".into()) });
        assert_eq!(parse("/mode yolo"), usage("mode"));
        assert_eq!(parse("/模式 3 默认"), Cmd::Mode { no: Some(3), mode: Some("default".into()) });
    }

    #[test]
    fn suggests_the_closest_command() {
        assert_eq!(parse("/stauts"), Cmd::Unknown("/stauts".into(), Some("status")));
        assert_eq!(parse("/efort high"), Cmd::Unknown("/efort".into(), Some("effort")));
        assert_eq!(parse("/lst"), Cmd::Unknown("/lst".into(), Some("ls")));
        assert_eq!(parse("/hel"), Cmd::Unknown("/hel".into(), Some("help")));
        assert_eq!(parse("/hello"), Cmd::Unknown("/hello".into(), None));
        assert_eq!(parse("/x"), Cmd::Unknown("/x".into(), None));
        assert_eq!(parse("/whatever"), Cmd::Unknown("/whatever".into(), None));
        assert_eq!(distance("kitten", "sitting"), 3);
        assert_eq!(distance("状态", "状太"), 1);
    }

    #[test]
    fn every_name_is_unique() {
        let mut seen = std::collections::HashSet::new();
        for s in SPECS {
            for n in s.names {
                assert!(seen.insert(*n), "{n} twice");
                assert_eq!(spec(&format!("/{}", n.to_uppercase())), Some(s));
            }
        }
    }

    #[test]
    fn parses_model_commands() {
        assert_eq!(parse("/model"), Cmd::Model(ModelCmd::Show));
        assert_eq!(parse("/model 2"), Cmd::Model(ModelCmd::Set("2".into())));
        assert_eq!(parse("/m gpt-6-luna"), Cmd::Model(ModelCmd::Set("gpt-6-luna".into())));
        assert_eq!(parse("/model default"), Cmd::Model(ModelCmd::Default(None)));
        assert_eq!(parse("/model default 3"), Cmd::Model(ModelCmd::Default(Some("3".into()))));
        assert_eq!(parse("/模型 默认 gpt-6-sol"), Cmd::Model(ModelCmd::Default(Some("gpt-6-sol".into()))));
        assert_eq!(parse("/effort"), Cmd::Effort(None));
        assert_eq!(parse("/effort high"), Cmd::Effort(Some("high".into())));
        assert_eq!(parse("/推理 默认"), Cmd::Effort(Some("default".into())));
        // To another session: the text is parsed again by the bridge.
        assert_eq!(parse("#3 /model 2"), Cmd::To { no: 3, force: false, text: "/model 2".into() });
    }

    #[test]
    fn aims_commands_at_a_session() {
        let at = |s: &str| for_session(parse(s), 3);
        assert_eq!(at("/stop"), Some(Cmd::Stop(Some(3))));
        assert_eq!(at("/queue clear"), Some(Cmd::Queue { no: Some(3), clear: true }));
        assert_eq!(at("/mode auto"), Some(Cmd::Mode { no: Some(3), mode: Some("auto".into()) }));
        assert_eq!(at("/rename New title"), Some(Cmd::Rename { no: Some(3), title: "New title".into() }));
        assert_eq!(at("/archive"), Some(Cmd::Archive(Some(3))));
        assert_eq!(at("/use"), Some(Cmd::Use { no: 3, force: false }));
        assert_eq!(at("/model 2"), Some(Cmd::Model(ModelCmd::Set("2".into()))));
        // A number given explicitly isn't overridden; the command isn't sent as text.
        assert_eq!(at("/stop 2"), None);
        assert_eq!(at("/ls"), None);
        assert_eq!(at("/status"), None);
        assert_eq!(at("/new"), None);
        assert_eq!(at("/foo"), Some(Cmd::Unknown("/foo".into(), None)));
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
