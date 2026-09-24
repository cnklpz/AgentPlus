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
    EN.load(Ordering::Relaxed)
}

/// Pick the text for the current language.
pub fn l(zh: &'static str, en: &'static str) -> &'static str {
    if is_en() { en } else { zh }
}

/// `format!` in the current language: `tr!("找不到 {id}", "Not found: {id}")`.
macro_rules! tr {
    ($zh:literal, $en:literal $(, $($arg:tt)*)?) => {
        if $crate::i18n::is_en() { format!($en $(, $($arg)*)?) } else { format!($zh $(, $($arg)*)?) }
    };
}

#[cfg(test)]
mod tests {
    #[test]
    fn picks_language() {
        let id = "x";
        assert_eq!(tr!("找不到 {id}", "Not found: {id}"), "找不到 x");
        assert_eq!(tr!("共 {} 个", "{} in total", 3), "共 3 个");
        assert_eq!(super::l("是", "yes"), "是");
    }
}
