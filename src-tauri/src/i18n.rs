//! UI language for text the backend produces (labels, notes, diff lines, errors).
//! The frontend sets it with `set_locale` before its first other call, and again
//! whenever the user switches language. Chinese is the source language and the default.
//!
//! - `l("中文", "English")` → `&'static str`, for text without placeholders.
//! - `tr!("找不到 {id}", "Not found: {id}")` → `String`, `format!` rules (escape `{{ }}`).

use std::sync::atomic::{AtomicBool, Ordering};

static EN: AtomicBool = AtomicBool::new(false);

/// "en…" selects English; anything else Chinese.
pub fn set(lang: &str) {
    EN.store(lang.starts_with("en"), Ordering::Relaxed);
}

pub fn is_en() -> bool {
    #[cfg(test)]
    if let Some(en) = TEST_EN.with(|t| t.get()) {
        return en;
    }
    EN.load(Ordering::Relaxed)
}

#[cfg(test)]
thread_local! {
    /// Tests: the language of this thread only (other tests keep the Chinese default).
    static TEST_EN: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Pick the text for the current language.
pub fn l(zh: &'static str, en: &'static str) -> &'static str {
    if is_en() { en } else { zh }
}

/// A list in running text: "a、b" / "a, b".
pub fn join<S: AsRef<str>>(items: &[S]) -> String {
    items.iter().map(|s| s.as_ref()).collect::<Vec<_>>().join(l("、", ", "))
}

/// A switch's state: "开" / "On".
pub fn on_off(b: bool) -> &'static str {
    if b { l("开", "On") } else { l("关", "Off") }
}

/// `format!` in the current language: `tr!("找不到 {id}", "Not found: {id}")`.
macro_rules! tr {
    ($zh:literal, $en:literal $(, $($arg:tt)*)?) => {
        if $crate::i18n::is_en() { format!($en $(, $($arg)*)?) } else { format!($zh $(, $($arg)*)?) }
    };
}

/// `tr!` with a count and an English singular / plural:
/// `trn!(n, "{n} 个模型", "{n} model", "{n} models")`. Chinese has one form; English takes
/// the singular when the count is 1. Every text writes the count as `{n}`; other
/// arguments follow as in `format!`.
macro_rules! trn {
    ($n:expr, $zh:literal, $one:literal, $many:literal $(, $($arg:tt)*)?) => {
        match $n {
            n if !$crate::i18n::is_en() => format!($zh $(, $($arg)*)?, n = n),
            1 => format!($one $(, $($arg)*)?, n = 1),
            n => format!($many $(, $($arg)*)?, n = n),
        }
    };
}

#[cfg(test)]
mod tests {
    use super::{join, on_off, TEST_EN};

    #[test]
    fn picks_language() {
        let id = "x";
        assert_eq!(tr!("找不到 {id}", "Not found: {id}"), "找不到 x");
        assert_eq!(tr!("共 {} 个", "{} in total", 3), "共 3 个");
        assert_eq!(super::l("是", "yes"), "是");
    }

    #[test]
    fn plurals_joins_and_switches() {
        let who = "R";
        let text = |n: usize| trn!(n, "「{}」{n} 个模型（{who}）", "\"{}\" {n} model ({who})", "\"{}\" {n} models ({who})", "p");
        assert_eq!((text(1), text(2), text(0)), ("「p」1 个模型（R）".into(), "「p」2 个模型（R）".into(), "「p」0 个模型（R）".into()));
        assert_eq!((join(&["a", "b"]), join::<String>(&[]), on_off(true), on_off(false)), ("a、b".into(), String::new(), "开", "关"));
        TEST_EN.with(|t| t.set(Some(true)));
        assert_eq!((text(1), text(2), text(0)), ("\"p\" 1 model (R)".into(), "\"p\" 2 models (R)".into(), "\"p\" 0 models (R)".into()));
        assert_eq!((join(&["a".to_string(), "b".to_string()]), on_off(true)), ("a, b".into(), "On"));
        TEST_EN.with(|t| t.set(None));
    }
}
