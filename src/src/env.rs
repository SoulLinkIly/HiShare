//! 环境变量读取（带缓存），用于调试开关与参数覆盖。

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Mutex, OnceLock};

fn cache() -> &'static Mutex<HashMap<&'static str, Option<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<&'static str, Option<String>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 读一个环境变量：先按本名读，读不到再退回 1.8.0 及更早的 `HWSHARE_` 前缀。
///
/// 改名前的老变量（`HWSHARE_CONFIRM` 之类）仍按原样生效，省得升级后
/// 原来的脚本、快捷方式、命令行参数集体失灵。
pub fn read(name: &str) -> Option<String> {
    if let Ok(value) = std::env::var(name) {
        return Some(value);
    }
    let legacy = name.strip_prefix("HISHARE_").map(|rest| format!("HWSHARE_{rest}"))?;
    std::env::var(legacy).ok()
}

pub fn text(name: &'static str) -> Option<String> {
    let mut guard = match cache().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard
        .entry(name)
        .or_insert_with(|| read(name))
        .clone()
}

fn with_cached<R>(name: &'static str, f: impl FnOnce(Option<&str>) -> R) -> R {
    let mut guard = match cache().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let cached = guard.entry(name).or_insert_with(|| read(name));
    f(cached.as_deref())
}

pub fn present(name: &'static str) -> bool {
    with_cached(name, |value| value.is_some())
}

pub fn flag(name: &'static str) -> bool {
    with_cached(name, |value| {
        value
            .map(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false)
    })
}

pub fn unless_zero(name: &'static str) -> bool {
    with_cached(name, |value| value.map(|value| value.trim() != "0").unwrap_or(true))
}

pub fn num<T: FromStr>(name: &'static str, default: T) -> T {
    with_cached(name, |value| {
        value
            .and_then(|value| value.trim().parse::<T>().ok())
            .unwrap_or(default)
    })
}
