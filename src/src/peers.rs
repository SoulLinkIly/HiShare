use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct PeerInfo {

    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub kind: String,
    pub seen: Instant,

    pub gap: Duration,
    pub accepts: bool,
}

static PEERS: Mutex<Vec<PeerInfo>> = Mutex::new(Vec::new());

static WANTED: Mutex<Vec<PeerInfo>> = Mutex::new(Vec::new());

static AUTO: Mutex<Option<PeerInfo>> = Mutex::new(None);

static REVISION: AtomicU64 = AtomicU64::new(0);

static LOADED: AtomicBool = AtomicBool::new(false);

fn targets_path() -> std::path::PathBuf {
    crate::store::path("targets.json")
}

fn same_device(a: &PeerInfo, b: &PeerInfo) -> bool {
    if !a.id.is_empty() && !b.id.is_empty() {
        a.id == b.id
    } else {
        a.host == b.host
    }
}

pub fn short_id(id: &str) -> &str {
    match id.char_indices().nth(12) {
        Some((index, _)) => &id[..index],
        None => id,
    }
}

fn targets_json(list: &[PeerInfo]) -> String {
    let items: Vec<serde_json::Value> = list
        .iter()
        .map(|p| serde_json::json!({ "id": p.id, "name": p.name, "host": p.host }))
        .collect();
    serde_json::to_string_pretty(&serde_json::json!({ "targets": items }))
        .unwrap_or_else(|_| "{\"targets\":[]}".to_string())
}

fn parse_targets(text: &str) -> Vec<PeerInfo> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let Some(items) = value.get("targets").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let now = Instant::now();

    let stale = now
        .checked_sub(ONLINE_WINDOW + Duration::from_secs(1))
        .unwrap_or(now);
    items
        .iter()
        .filter_map(|item| {
            let host = item.get("host")?.as_str()?.trim().to_string();
            if host.is_empty() {
                return None;
            }
            let name = item
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .unwrap_or(&host)
                .to_string();

            let id = item
                .get("id")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("")
                .to_string();
            Some(PeerInfo {
                id,
                name,
                host,
                port: 0,
                kind: String::new(),
                seen: stale,
                gap: Duration::ZERO,
                accepts: true,
            })
        })
        .collect()
}

fn save_targets() {

    if cfg!(test) {
        return;
    }
    let Ok(list) = WANTED.lock() else { return };
    let text = targets_json(&list);
    drop(list);
    let path = targets_path();

    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

fn load_saved_targets() -> Vec<PeerInfo> {
    if cfg!(test) {
        return Vec::new();
    }
    match std::fs::read_to_string(targets_path()) {
        Ok(text) => parse_targets(&text),
        Err(_) => Vec::new(),
    }
}

fn ensure_loaded() {
    if LOADED.swap(true, Ordering::SeqCst) {
        return;
    }
    let saved = load_saved_targets();
    if saved.is_empty() {
        return;
    }
    if let Ok(mut list) = WANTED.lock() {
        if list.is_empty() {
            *list = saved;
        }
    }
    bump();
}

pub fn auto_picked() -> bool {
    AUTO.lock().map(|slot| slot.is_some()).unwrap_or(false)
}

static USER_CLEARED: AtomicBool = AtomicBool::new(false);

fn bump() {
    REVISION.fetch_add(1, Ordering::SeqCst);
}

pub const ONLINE_WINDOW: Duration = Duration::from_secs(15);
const ONLINE_WINDOW_FLOOR: Duration = Duration::from_secs(4);
const RHYTHM_MEMORY: Duration = Duration::from_secs(60);
const GAP_WEIGHT: u32 = 3;

pub fn offline_after(peer: &PeerInfo) -> Duration {
    if peer.gap.is_zero() {
        return ONLINE_WINDOW;
    }
    (peer.gap * 3).clamp(ONLINE_WINDOW_FLOOR, ONLINE_WINDOW)
}

fn next_gap(previous: Duration, observed: Duration) -> Duration {
    if observed > RHYTHM_MEMORY {
        return Duration::ZERO;
    }
    if previous.is_zero() {
        return observed;
    }
    (previous * GAP_WEIGHT + observed) / (GAP_WEIGHT + 1)
}

pub fn online(peer: &PeerInfo) -> bool {
    peer.seen.elapsed() <= offline_after(peer) && peer.accepts
}

fn online_match(want: &PeerInfo) -> Option<PeerInfo> {
    PEERS
        .lock()
        .ok()?
        .iter()
        .find(|p| online(p) && same_device(want, p))
        .cloned()
}

pub fn upsert(id: &str, name: &str, host: &str, port: u16, kind: &str, accepts: bool) {
    if crate::env::present("HISHARE_DEBUG") {
        use std::io::Write;
        let line = format!("{name}\t{host}:{port}\t{kind}\t{id}\n");
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(std::env::temp_dir().join("hishare-peers.log"))
        {
            let _ = file.write_all(line.as_bytes());
        }
    }
    let Ok(mut list) = PEERS.lock() else { return };

    let mut index = if id.is_empty() {
        None
    } else {
        list.iter().position(|p| p.id == id)
    };
    let mut replaced: Option<String> = None;
    if index.is_none() {
        if let Some(position) = list.iter().position(|p| p.host == host) {

            if id.is_empty() || list[position].id.is_empty() || list[position].id == id {
                index = Some(position);
            } else {

                replaced = Some(list[position].name.clone());
                list.remove(position);
            }
        }
    }

    if let Some(index) = index {

        let slot = &mut list[index];
        let mut changed = false;
        if !id.is_empty() && slot.id != id {
            slot.id.clear();
            slot.id.push_str(id);
            changed = true;
        }
        if slot.name != name {
            slot.name.clear();
            slot.name.push_str(name);
            changed = true;
        }

        if slot.host != host {
            slot.host.clear();
            slot.host.push_str(host);
            changed = true;
        }
        if slot.port != port {
            slot.port = port;
            changed = true;
        }
        if slot.kind != kind {
            slot.kind.clear();
            slot.kind.push_str(kind);
            changed = true;
        }
        if slot.accepts != accepts {
            slot.accepts = accepts;
            changed = true;
        }
        let now = Instant::now();
        let gap = now.saturating_duration_since(slot.seen);
        slot.gap = next_gap(slot.gap, gap);
        slot.seen = now;
        if changed {
            bump();
        }
        drop(list);
        if let Some(previous) = replaced {
            #[cfg(windows)]
            crate::tray::log_line(&format!(
                "[发现] {} 的地址 {} 已属于另一台设备，旧记录已移除",
                previous, host
            ));
        }
        return;
    }
    if list.len() >= 32 {

        if let Some(index) = list
            .iter()
            .enumerate()
            .max_by_key(|(_, peer)| peer.seen.elapsed())
            .map(|(index, _)| index)
        {
            list.remove(index);
        }
    }
    list.push(PeerInfo {
        id: id.to_string(),
        name: name.to_string(),
        host: host.to_string(),
        port,
        kind: kind.to_string(),
        seen: Instant::now(),
        gap: Duration::ZERO,
        accepts,
    });
    bump();
}

pub fn snapshot() -> Vec<PeerInfo> {
    PEERS.lock().map(|list| list.clone()).unwrap_or_default()
}

pub fn wanted() -> Vec<PeerInfo> {
    ensure_loaded();
    let Ok(mut slot) = WANTED.lock() else {
        return Vec::new();
    };
    if slot.is_empty() {
        return Vec::new();
    }
    let live = PEERS.lock().map(|list| list.clone()).unwrap_or_default();

    let mut learned: Vec<String> = Vec::new();
    slot.retain_mut(|want| {

        let fresh = live.iter().find(|p| {
            if !want.id.is_empty() {
                !p.id.is_empty() && p.id == want.id
            } else {
                p.host == want.host
            }
        });
        let Some(fresh) = fresh else {

            return true;
        };
        if want.id.is_empty() && !fresh.id.is_empty() {
            learned.push(format!(
                "{} → {}",
                fresh.name,
                short_id(&fresh.id)
            ));
            want.id = fresh.id.clone();
        }
        want.host = fresh.host.clone();
        want.port = fresh.port;
        want.accepts = fresh.accepts;
        want.seen = fresh.seen;
        want.gap = fresh.gap;
        if !fresh.name.is_empty() && want.name != fresh.name {
            want.name = fresh.name.clone();
        }
        if want.kind != fresh.kind {
            want.kind = fresh.kind.clone();
        }
        true
    });
    let list = slot.clone();
    drop(slot);

    if !learned.is_empty() {
        bump();
        save_targets();
        #[cfg(windows)]
        crate::tray::log_line(&format!(
            "[目标] 已记住设备标识（以后换 IP 也认得住）：{}",
            learned.join("、")
        ));
    }
    list
}

fn auto_peer() -> Option<PeerInfo> {
    let auto = AUTO.lock().ok()?.clone()?;
    online_match(&auto)
}

pub fn selected() -> Vec<PeerInfo> {
    let wants = wanted();
    if wants.is_empty() {

        return auto_peer().into_iter().collect();
    }

    wants.iter().filter_map(online_match).collect()
}

pub fn is_target(peer: &PeerInfo) -> bool {
    wanted().iter().any(|p| same_device(p, peer))
}

pub fn toggle_target(peer: &PeerInfo) -> bool {

    let auto_hit = AUTO
        .lock()
        .map(|slot| {
            slot.as_ref()
                .map(|p| same_device(p, peer))
                .unwrap_or(false)
        })
        .unwrap_or(false);
    if auto_hit {
        if let Ok(mut slot) = AUTO.lock() {
            *slot = None;
        }
        USER_CLEARED.store(true, Ordering::SeqCst);
        bump();
        return false;
    }

    let (on, empty) = {
        let Ok(mut list) = WANTED.lock() else {
            return false;
        };
        let on = match list.iter().position(|p| same_device(p, peer)) {
            Some(index) => {
                list.remove(index);
                false
            }
            None => {
                list.push(peer.clone());
                true
            }
        };
        (on, list.is_empty())
    };

    if on {
        if let Ok(mut slot) = AUTO.lock() {
            *slot = None;
        }
    }

    USER_CLEARED.store(!on && empty, Ordering::SeqCst);
    bump();
    save_targets();
    on
}

pub fn clear_targets() {
    if let Ok(mut list) = WANTED.lock() {
        list.clear();
    }
    if let Ok(mut slot) = AUTO.lock() {
        *slot = None;
    }
    USER_CLEARED.store(true, Ordering::SeqCst);
    bump();
    save_targets();
}

pub fn prune_offline() -> usize {
    let mut devices = 0;
    if let Ok(mut list) = PEERS.lock() {
        let before = list.len();
        list.retain(online);
        devices = before - list.len();
    }
    if devices > 0 {
        bump();
    }
    devices
}

#[derive(Clone, Debug)]
pub struct TargetView {
    pub peers: Vec<PeerInfo>,
    pub selected: Vec<PeerInfo>,
    pub revision: u64,
}

fn ordered(mut peers: Vec<PeerInfo>) -> Vec<PeerInfo> {
    peers.sort_by(|a, b| {
        let a_online = online(a);
        let b_online = online(b);
        b_online
            .cmp(&a_online)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.host.cmp(&b.host))
    });
    peers
}

fn place_selected_first(mut peers: Vec<PeerInfo>, selected: &[PeerInfo]) -> Vec<PeerInfo> {
    if selected.is_empty() {
        return peers;
    }
    let mut front: Vec<PeerInfo> = Vec::with_capacity(selected.len());
    for pick in selected {
        match peers.iter().position(|p| same_device(p, pick)) {
            Some(index) => front.push(peers.remove(index)),
            None => front.push(pick.clone()),
        }
    }
    front.extend(peers);
    front
}

fn try_auto_pick(peers: &[PeerInfo]) -> Option<PeerInfo> {
    if !wanted().is_empty() || USER_CLEARED.load(Ordering::SeqCst) {
        return None;
    }
    if AUTO.lock().map(|slot| slot.is_some()).unwrap_or(false) {
        return None;
    }
    let pick = peers.iter().find(|p| online(p))?.clone();
    if let Ok(mut slot) = AUTO.lock() {
        *slot = Some(pick.clone());
    }

    crate::report::log(&format!(
        "[目标] 还没选过接收方，先替你勾上「{}」，可在托盘菜单里改",
        pick.name
    ));
    bump();
    Some(pick)
}

pub fn view() -> TargetView {
    let sorted = ordered(snapshot());
    let mut picks = selected();
    if picks.is_empty() {
        if let Some(pick) = try_auto_pick(&sorted) {
            picks = vec![pick];
        }
    }
    let peers = place_selected_first(sorted, &picks);
    TargetView {
        peers,
        selected: picks,
        revision: revision(),
    }
}

pub fn revision() -> u64 {
    REVISION.load(Ordering::SeqCst)
}

pub fn name_for_host(host: &str) -> Option<String> {
    let ip = host.split(':').next().unwrap_or(host);
    let list = PEERS.lock().ok()?;
    list.iter()
        .find(|p| p.host == ip || p.host == host)
        .map(|p| p.name.clone())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    Open,
    Closed,
}

static VERDICTS: Mutex<Vec<(String, Verdict, Instant)>> = Mutex::new(Vec::new());

const VERDICT_TTL: Duration = Duration::from_secs(180);

pub fn mark_verdict(host: &str, verdict: Verdict) {
    let Ok(mut list) = VERDICTS.lock() else {
        return;
    };
    list.retain(|(h, _, at)| h != host && at.elapsed() < VERDICT_TTL);
    list.push((host.to_string(), verdict, Instant::now()));
}

pub fn verdict(host: &str) -> Option<Verdict> {
    let Ok(mut list) = VERDICTS.lock() else {
        return None;
    };
    list.retain(|(_, _, at)| at.elapsed() < VERDICT_TTL);
    list.iter()
        .find(|(h, _, _)| h == host)
        .map(|(_, v, _)| *v)
}

#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
pub(crate) fn reset_for_test() {
    if let Ok(mut list) = PEERS.lock() {
        list.clear();
    }
    if let Ok(mut list) = WANTED.lock() {
        list.clear();
    }
    if let Ok(mut slot) = AUTO.lock() {
        *slot = None;
    }

    USER_CLEARED.store(true, Ordering::SeqCst);
    bump();
}

#[cfg(test)]
pub(crate) fn upsert_aged(
    id: &str,
    name: &str,
    host: &str,
    port: u16,
    kind: &str,
    age: Duration,
) {
    upsert(id, name, host, port, kind, true);
    if let Ok(mut list) = PEERS.lock() {
        if let Some(peer) = list.iter_mut().find(|p| {
            if id.is_empty() {
                p.host == host
            } else {
                p.id == id
            }
        }) {
            peer.seen = Instant::now().checked_sub(age).unwrap_or_else(Instant::now);
            peer.gap = Duration::ZERO;
        }
    }
    bump();
}

#[cfg(test)]
mod tests {
    use super::*;

    const OVER_OFFLINE: Duration = Duration::from_secs(16);

    fn peer(name: &str, host: &str, age: Duration) -> PeerInfo {
        PeerInfo {
            id: String::new(),
            name: name.to_string(),
            host: host.to_string(),
            port: 1234,
            kind: "phone".to_string(),
            seen: Instant::now().checked_sub(age).unwrap_or_else(Instant::now),
            gap: Duration::ZERO,
            accepts: true,
        }
    }

    fn reset() {
        reset_for_test();
    }

    #[test]
    fn new_peer_never_steals_target() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        upsert("", "手机A", "10.0.0.1", 1234, "phone", true);
        let picked = snapshot()
            .into_iter()
            .find(|p| p.host == "10.0.0.1")
            .expect("A 应该在列表里");
        assert!(toggle_target(&picked));
        assert_eq!(
            selected().first().map(|p| p.host.clone()),
            Some("10.0.0.1".to_string())
        );

        upsert("", "手机B", "10.0.0.2", 5678, "phone", true);
        assert_eq!(
            selected().iter().map(|p| p.host.as_str()).collect::<Vec<_>>(),
            vec!["10.0.0.1"],
            "新设备不能被自动加进发送目标"
        );

        upsert("", "手机A", "10.0.0.1", 9999, "phone", true);
        assert_eq!(selected().first().map(|p| p.port), Some(9999));
        reset();
    }

    #[test]
    fn toggle_target_adds_and_removes() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let a = peer("A", "10.9.0.1", Duration::ZERO);
        let b = peer("B", "10.9.0.2", Duration::ZERO);
        upsert("", "A", "10.9.0.1", 1234, "phone", true);
        upsert("", "B", "10.9.0.2", 1234, "tablet", true);

        assert!(toggle_target(&a), "第一次点应该是勾上");
        assert!(toggle_target(&b));
        assert_eq!(selected().len(), 2, "多选：两台都要在");
        assert!(is_target(&b));

        assert!(!toggle_target(&a), "再点一次是取消");
        assert_eq!(selected().len(), 1);
        assert!(!is_target(&a));
        assert!(!USER_CLEARED.load(Ordering::SeqCst), "还有一台勾着，不算清空");

        assert!(!toggle_target(&b));
        assert!(selected().is_empty());
        assert!(wanted().is_empty());
        assert!(USER_CLEARED.load(Ordering::SeqCst), "勾掉最后一台 = 明确不要目标");
        reset();
    }

    #[test]
    fn offline_target_is_deselected_but_remembered() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        upsert("", "手机A", "10.9.1.1", 1234, "phone", true);
        let a = snapshot().into_iter().find(|p| p.host == "10.9.1.1").unwrap();
        assert!(toggle_target(&a));
        assert_eq!(selected().len(), 1);

        upsert_aged("", "手机A", "10.9.1.1", 1234, "phone", OVER_OFFLINE);
        assert!(selected().is_empty(), "离线就不算选中");
        assert_eq!(wanted().len(), 1, "意图要留着，等它回来");
        assert!(
            snapshot().iter().any(|p| p.host == "10.9.1.1"),
            "设备还在列表里（灰着）"
        );
        assert!(!USER_CLEARED.load(Ordering::SeqCst), "掉线不是用户改主意");
        reset();
    }

    #[test]
    fn chatty_peer_goes_offline_after_four_missed_beacons() {
        let mut p = peer("手机A", "10.9.2.1", Duration::from_millis(3500));
        p.gap = Duration::from_secs(1);
        assert_eq!(offline_after(&p), Duration::from_secs(4), "1 Hz 广播只等 4 s");
        assert!(online(&p), "3.5 s 没包，还在窗口内");

        p.seen = Instant::now()
            .checked_sub(Duration::from_millis(4200))
            .unwrap_or_else(Instant::now);
        assert!(!online(&p), "漏掉 4 个包就该变灰");
    }

    #[test]
    fn unknown_rhythm_keeps_the_long_window() {
        let p = peer("手机A", "10.9.2.2", Duration::from_secs(10));
        assert_eq!(offline_after(&p), ONLINE_WINDOW, "没学到节奏时沿用 15 s");
        assert!(online(&p));
        assert!(!online(&peer("手机A", "10.9.2.2", OVER_OFFLINE)));
    }

    #[test]
    fn slow_rhythm_scales_the_window() {
        let mut p = peer("手机A", "10.9.2.3", Duration::ZERO);
        p.gap = Duration::from_secs(3);
        assert_eq!(offline_after(&p), Duration::from_secs(9), "3 s 一包就等 3 包");
        p.gap = Duration::from_secs(8);
        assert_eq!(offline_after(&p), ONLINE_WINDOW, "稀疏广播不许超过 15 s");
    }

    #[test]
    fn rhythm_smooths_towards_the_latest_gap() {
        assert_eq!(
            next_gap(Duration::ZERO, Duration::from_secs(1)),
            Duration::from_secs(1),
            "第一包直接采信"
        );
        assert_eq!(
            next_gap(Duration::from_secs(1), Duration::from_secs(9)),
            Duration::from_secs(3),
            "一次抖动只拉高四分之一"
        );
        assert_eq!(
            next_gap(Duration::from_secs(1), Duration::from_secs(90)),
            Duration::ZERO,
            "隔了一分钟以上不算节奏，下次重新学"
        );
    }

    #[test]
    fn close_share_turns_target_offline_before_timeout() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        upsert("", "手机A", "10.9.3.1", 1234, "phone", true);
        let first = snapshot().into_iter().find(|p| p.host == "10.9.3.1").unwrap();
        toggle_target(&first);
        assert_eq!(selected().len(), 1);

        upsert("", "手机A", "10.9.3.1", 1234, "phone", false);
        assert!(selected().is_empty(), "关分享的回包 cap 无 256 位，当场掉选");
        assert_eq!(wanted().len(), 1, "意图仍在，重开分享自动勾回");

        upsert("", "手机A", "10.9.3.1", 1234, "phone", true);
        assert_eq!(selected().len(), 1, "重新允许接收后自动回到选中");
        reset();
    }

    #[test]
    fn target_comes_back_online_and_is_reselected() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        upsert("", "手机A", "10.9.1.7", 1234, "phone", true);
        let a = snapshot().into_iter().find(|p| p.host == "10.9.1.7").unwrap();
        toggle_target(&a);
        upsert_aged("", "手机A", "10.9.1.7", 1234, "phone", OVER_OFFLINE);
        assert!(selected().is_empty());

        upsert("", "手机A", "10.9.1.7", 9999, "phone", true);
        let picks = selected();
        assert_eq!(picks.len(), 1, "回上线要自动选回");
        assert_eq!(picks[0].port, 9999, "端口要是最新的");
        assert!(!auto_picked(), "这是用户自己点过的，不该标「自动」");
        reset();
    }

    #[test]
    fn multi_target_only_offline_one_is_dropped() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        upsert("", "手机A", "10.9.2.1", 1234, "phone", true);
        upsert("", "平板B", "10.9.2.2", 5678, "tablet", true);
        for host in ["10.9.2.1", "10.9.2.2"] {
            let p = snapshot().into_iter().find(|p| p.host == host).unwrap();
            toggle_target(&p);
        }
        assert_eq!(selected().len(), 2);

        upsert_aged("", "手机A", "10.9.2.1", 1234, "phone", OVER_OFFLINE);
        assert_eq!(
            selected().iter().map(|p| p.host.as_str()).collect::<Vec<_>>(),
            vec!["10.9.2.2"],
            "只掉离线那台"
        );
        assert_eq!(wanted().len(), 2, "两台都还记着");
        assert!(!USER_CLEARED.load(Ordering::SeqCst));
        reset();
    }

    #[test]
    fn offline_peer_stays_in_list_without_time_limit() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        upsert_aged("", "老设备", "10.9.2.9", 1234, "phone", Duration::from_secs(86400));
        assert!(
            snapshot().iter().any(|p| p.host == "10.9.2.9"),
            "一天没消息也不能自己消失"
        );
        assert!(selected().is_empty(), "不在线就不能发");
        reset();
    }

    #[test]
    fn ordered_prefers_online_then_name() {
        let list = ordered(vec![
            peer("B", "10.0.0.2", Duration::from_secs(5)),
            peer("A", "10.0.0.1", Duration::from_secs(600)),
            peer("C", "10.0.0.3", Duration::from_secs(5)),
        ]);
        let names: Vec<&str> = list.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["B", "C", "A"], "在线优先，同组按名字");
    }

    #[test]
    fn share_off_peer_counts_offline() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        upsert("", "关了分享", "10.9.9.1", 1234, "phone", false);
        let p = snapshot()
            .into_iter()
            .find(|p| p.host == "10.9.9.1")
            .expect("设备应该在列表里");
        assert!(!online(&p), "有包但不接收就是离线");
        assert!(selected().is_empty(), "不该进发送目标");
        assert!(snapshot().iter().any(|p| p.host == "10.9.9.1"), "设备还留着，灰着");
        reset();
    }

    #[test]
    fn share_off_target_is_deselected_until_share_back() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        upsert("", "手机A", "10.9.9.2", 1234, "phone", true);
        let a = snapshot()
            .into_iter()
            .find(|p| p.host == "10.9.9.2")
            .unwrap();
        assert!(toggle_target(&a));
        assert_eq!(selected().len(), 1);

        upsert("", "手机A", "10.9.9.2", 1234, "phone", false);
        assert!(selected().is_empty(), "关了分享立刻掉选");
        assert_eq!(wanted().len(), 1, "意图要留着");
        assert!(!USER_CLEARED.load(Ordering::SeqCst), "关分享不是用户改主意");

        upsert("", "手机A", "10.9.9.2", 9999, "phone", true);
        let picks = selected();
        assert_eq!(picks.len(), 1, "重开分享自动选回");
        assert_eq!(picks[0].port, 9999, "端口用最新的");
        reset();
    }

    #[test]
    fn multi_target_drops_only_the_one_that_stops_accepting() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        upsert("", "手机A", "10.9.9.3", 1234, "phone", true);
        upsert("", "平板B", "10.9.9.4", 5678, "tablet", true);
        for host in ["10.9.9.3", "10.9.9.4"] {
            let p = snapshot().into_iter().find(|p| p.host == host).unwrap();
            toggle_target(&p);
        }
        assert_eq!(selected().len(), 2);

        upsert("", "手机A", "10.9.9.3", 1234, "phone", false);
        assert_eq!(
            selected().iter().map(|p| p.host.as_str()).collect::<Vec<_>>(),
            vec!["10.9.9.4"],
            "只掉关分享那台"
        );
        assert_eq!(wanted().len(), 2, "两台都还记着");
        reset();
    }

    #[test]
    fn auto_pick_skips_peer_that_does_not_accept() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        USER_CLEARED.store(false, Ordering::SeqCst);
        upsert("", "关了分享", "10.9.9.5", 1234, "phone", false);
        assert!(view().selected.is_empty(), "不接收的设备不该被代选");
        assert!(!auto_picked(), "没选过就保持未选择");

        upsert("", "开着分享", "10.9.9.6", 5678, "phone", true);
        assert_eq!(
            view().selected.iter().map(|p| p.host.as_str()).collect::<Vec<_>>(),
            vec!["10.9.9.6"],
            "代选要跳过不接收的那台"
        );
        reset();
    }

    #[test]
    fn rescan_prunes_peer_that_stopped_accepting() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        upsert("", "手机A", "10.9.9.7", 1234, "phone", true);
        let a = snapshot()
            .into_iter()
            .find(|p| p.host == "10.9.9.7")
            .unwrap();
        toggle_target(&a);
        upsert("", "手机A", "10.9.9.7", 1234, "phone", false);

        assert_eq!(prune_offline(), 1, "关分享的设备也按离线清掉");
        assert!(!snapshot().iter().any(|p| p.host == "10.9.9.7"));
        assert!(
            wanted().iter().any(|p| p.host == "10.9.9.7"),
            "关掉分享只是当下不可达，目标仍是用户选的，不该被重扫删掉"
        );
        assert!(selected().is_empty(), "不接收的设备不进可发送列表");
        reset();
    }

    #[test]
    fn ordered_puts_unaccepting_after_online() {
        let mut closed = peer("A", "10.0.0.1", Duration::ZERO);
        closed.accepts = false;
        let list = ordered(vec![closed, peer("B", "10.0.0.2", Duration::ZERO)]);
        assert_eq!(
            list.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["B", "A"],
            "不接收的排在线组之后，即使它刚回过包"
        );
    }

    #[test]
    fn auto_pick_happens_once_and_never_switches() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        USER_CLEARED.store(false, Ordering::SeqCst);

        upsert("", "手机A", "10.9.3.1", 1234, "phone", true);
        assert_eq!(view().selected.len(), 1, "从没表过态：代选一台在线设备");
        assert!(auto_picked(), "菜单上要标「自动」");

        upsert_aged("", "手机A", "10.9.3.1", 1234, "phone", OVER_OFFLINE);
        upsert("", "平板B", "10.9.3.2", 5678, "tablet", true);
        assert!(
            view().selected.is_empty(),
            "代选过就不再挑第二台，宁可空着"
        );
        assert!(auto_picked(), "还记着那台");

        upsert("", "手机A", "10.9.3.1", 1234, "phone", true);
        assert_eq!(
            view().selected.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["手机A"]
        );
        reset();
    }

    #[test]
    fn clicking_the_auto_picked_peer_unchecks_it() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        USER_CLEARED.store(false, Ordering::SeqCst);
        upsert("", "手机A", "10.9.8.1", 1234, "phone", true);
        let auto = view().selected[0].clone();
        assert!(auto_picked());

        assert!(!toggle_target(&auto), "点代选的那台 = 取消它");
        assert!(!auto_picked());
        assert!(USER_CLEARED.load(Ordering::SeqCst));
        assert!(selected().is_empty());
        assert!(wanted().is_empty(), "代选的那台不进意图表");
        reset();
    }

    #[test]
    fn rescan_prunes_only_offline_devices() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        upsert("", "手机A", "10.9.4.1", 1234, "phone", true);
        upsert("", "平板B", "10.9.4.2", 5678, "tablet", true);
        for host in ["10.9.4.1", "10.9.4.2"] {
            let p = snapshot().into_iter().find(|p| p.host == host).unwrap();
            toggle_target(&p);
        }
        upsert_aged("", "手机A", "10.9.4.1", 1234, "phone", OVER_OFFLINE);

        assert_eq!(prune_offline(), 1, "只清掉那一台离线设备");
        assert!(
            !snapshot().iter().any(|p| p.host == "10.9.4.1"),
            "离线设备从列表消失"
        );
        assert!(
            snapshot().iter().any(|p| p.host == "10.9.4.2"),
            "在线设备留着"
        );
        let kept = wanted();
        assert!(
            kept.iter().any(|p| p.host == "10.9.4.1"),
            "目标是用户选的，设备离线也要留着，不许顺手删"
        );
        assert!(kept.iter().any(|p| p.host == "10.9.4.2"));
        assert!(
            selected().iter().any(|p| p.host == "10.9.4.2"),
            "在线的还勾着"
        );
        assert!(!USER_CLEARED.load(Ordering::SeqCst), "重扫不是用户改主意");
        reset();
    }

    #[test]
    fn rescan_then_device_returns_still_selected() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        upsert("", "手机A", "10.9.5.1", 1234, "phone", true);
        let a = snapshot().into_iter().find(|p| p.host == "10.9.5.1").unwrap();
        toggle_target(&a);
        upsert_aged("", "手机A", "10.9.5.1", 1234, "phone", OVER_OFFLINE);
        assert_eq!(prune_offline(), 1);
        assert!(selected().is_empty(), "离线的时候确实没得发");

        upsert("", "手机A", "10.9.5.1", 1234, "phone", true);
        assert!(
            selected().iter().any(|p| p.host == "10.9.5.1"),
            "设备回来要自动恢复成目标，不用手动补勾"
        );
        reset();
    }

    #[test]
    fn rescan_keeps_auto_pick_alive() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        USER_CLEARED.store(false, Ordering::SeqCst);
        upsert_aged("", "手机A", "10.9.6.1", 1234, "phone", OVER_OFFLINE);
        assert_eq!(prune_offline(), 1);
        assert!(!USER_CLEARED.load(Ordering::SeqCst), "重扫不该关掉自动挑选");

        upsert("", "平板B", "10.9.6.2", 5678, "tablet", true);
        assert!(
            view().selected.iter().any(|p| p.host == "10.9.6.2"),
            "没选过目标时，设备回来照样自动挑一台"
        );
        reset();
    }

    #[test]
    fn reused_ip_does_not_steal_intent() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        upsert("id-a", "手机A", "10.9.7.1", 1234, "phone", true);
        let a = snapshot().into_iter().find(|p| p.host == "10.9.7.1").unwrap();
        toggle_target(&a);

        upsert("id-b", "别人的手机", "10.9.7.1", 5678, "phone", true);
        assert!(
            selected().is_empty(),
            "地址虽同、设备不同，绝不能把文件发过去"
        );
        assert_eq!(wanted().len(), 1, "旧目标要留着等手机A自己回来");
        assert_eq!(wanted()[0].id, "id-a", "意图记的是设备，不是地址");
        assert!(
            !snapshot().iter().any(|p| p.id == "id-a" && p.host == "10.9.7.1"),
            "被证伪的旧地址记录不该留着"
        );
        assert!(snapshot().iter().any(|p| p.id == "id-b"), "新设备进列表");
        reset();
    }

    #[test]
    fn ip_change_updates_one_entry() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        upsert("id-a", "手机A", "10.9.10.5", 43563, "phone", true);
        assert_eq!(snapshot().len(), 1);

        upsert("id-a", "手机A", "10.9.10.7", 45369, "phone", true);
        assert_eq!(snapshot().len(), 1, "换 IP 不能多出一条");
        let peer = snapshot().pop().unwrap();
        assert_eq!(peer.host, "10.9.10.7");
        assert_eq!(peer.port, 45369);
        reset();
    }

    #[test]
    fn intent_follows_device_across_ip_change() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        upsert("id-a", "手机A", "10.9.11.5", 43563, "phone", true);
        let a = snapshot().into_iter().find(|p| p.id == "id-a").unwrap();
        toggle_target(&a);

        upsert_aged("id-a", "手机A", "10.9.11.5", 43563, "phone", OVER_OFFLINE);
        assert!(selected().is_empty(), "离线就不算选中");

        upsert("id-a", "手机A", "10.9.11.7", 45369, "phone", true);
        let picks = selected();
        assert_eq!(picks.len(), 1, "换 IP 也要自动选回");
        assert_eq!(picks[0].host, "10.9.11.7", "发到新地址");
        assert_eq!(picks[0].port, 45369, "端口用最新的");
        reset();
    }

    #[test]
    fn legacy_target_learns_udid() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();

        let legacy = peer("手机A", "10.9.12.1", Duration::from_secs(3600));
        if let Ok(mut list) = WANTED.lock() {
            list.push(legacy);
        }
        assert!(selected().is_empty(), "刚启动、没收到包之前不能当成在线");

        upsert("id-a", "手机A", "10.9.12.1", 1234, "phone", true);
        assert_eq!(selected().len(), 1, "扫到就该认回这个目标");
        assert_eq!(wanted()[0].id, "id-a", "顺手把 UDID 学到手并落盘");

        upsert("id-a", "手机A", "10.9.12.99", 4321, "phone", true);
        assert_eq!(
            selected().first().map(|p| p.host.as_str()),
            Some("10.9.12.99")
        );
        reset();
    }

    #[test]
    fn same_name_devices_are_distinct() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        upsert("id-a", "手机", "10.9.13.1", 1234, "phone", true);
        upsert("id-b", "手机", "10.9.13.2", 1234, "phone", true);
        assert_eq!(snapshot().len(), 2, "同名不是同一台");

        let a = snapshot().into_iter().find(|p| p.id == "id-a").unwrap();
        toggle_target(&a);
        assert_eq!(selected().len(), 1);
        assert_eq!(selected()[0].id, "id-a");

        upsert("id-a", "手机", "10.9.13.9", 1234, "phone", true);
        assert_eq!(selected()[0].host, "10.9.13.9");
        assert_eq!(wanted()[0].id, "id-a");
        reset();
    }

    #[test]
    fn missing_udid_falls_back_to_host() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        upsert("", "无标识设备", "10.9.14.1", 1234, "phone", true);
        let p = snapshot().into_iter().find(|p| p.host == "10.9.14.1").unwrap();
        toggle_target(&p);
        assert_eq!(selected().len(), 1);
        assert_eq!(wanted()[0].id, "", "没标识就存空，别瞎编一个");

        upsert("", "无标识设备", "10.9.14.1", 9999, "phone", true);
        assert_eq!(selected().len(), 1, "同地址仍认作同一台");
        assert_eq!(selected()[0].port, 9999);
        reset();
    }

    #[test]
    fn full_table_drops_the_stalest_peer() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        for i in 0..32u16 {
            upsert("", &format!("设备{i}"), &format!("10.10.0.{i}"), 1000, "phone", true);
        }
        upsert_aged("", "设备0", "10.10.0.0", 1000, "phone", Duration::from_secs(3600));
        upsert("", "新设备", "10.10.0.99", 1000, "phone", true);

        assert!(
            !snapshot().iter().any(|p| p.host == "10.10.0.0"),
            "顶掉的必须是那台最久没消息的"
        );
        assert!(snapshot().iter().any(|p| p.host == "10.10.0.99"));
        reset();
    }

    #[test]
    fn selected_peers_are_moved_to_the_top() {
        let a = peer("A", "10.0.0.1", Duration::ZERO);
        let b = peer("B", "10.0.0.2", Duration::ZERO);
        let c = peer("C", "10.0.0.3", Duration::ZERO);
        let list = vec![a.clone(), b.clone(), c.clone()];

        let untouched = place_selected_first(list.clone(), &[]);
        assert_eq!(
            untouched.iter().map(|p| p.host.as_str()).collect::<Vec<_>>(),
            list.iter().map(|p| p.host.as_str()).collect::<Vec<_>>(),
            "没选目标时列表顺序不变"
        );
        let moved = place_selected_first(list.clone(), &[c.clone(), a.clone()]);
        assert_eq!(
            moved.iter().map(|p| p.host.as_str()).collect::<Vec<_>>(),
            vec![c.host.as_str(), a.host.as_str(), b.host.as_str()],
            "勾选的目标排最前，顺序跟勾选顺序一致"
        );
        assert_eq!(moved.len(), 3);

        let outsider = peer("Z", "10.9.9.9", Duration::ZERO);
        let added = place_selected_first(list, &[outsider.clone()]);
        assert_eq!(added.len(), 4);
        assert_eq!(added[0].host, outsider.host);
    }

    #[test]
    fn targets_json_round_trips() {
        let list = vec![
            peer("手机A", "10.7.0.1", Duration::ZERO),
            peer("平板B", "10.7.0.2", Duration::ZERO),
        ];
        let back = parse_targets(&targets_json(&list));
        assert_eq!(
            back.iter().map(|p| p.host.as_str()).collect::<Vec<_>>(),
            vec!["10.7.0.1", "10.7.0.2"],
            "顺序要保住：勾选顺序 = 目标顺序"
        );
        assert_eq!(back[0].name, "手机A");
        assert_eq!(back[0].port, 0, "端口不落盘，等发现包刷新");
    }

    #[test]
    fn parse_targets_survives_garbage() {
        assert!(parse_targets("").is_empty(), "空文件当没配过");
        assert!(parse_targets("not json").is_empty());
        assert!(
            parse_targets("{\"targets\":{}}").is_empty(),
            "类型不对也不许崩"
        );
        let mixed = r#"{"targets":[{"name":"","host":"10.7.1.1"},{"host":"  "},{"name":"只有名字"}]}"#;
        let list = parse_targets(mixed);
        assert_eq!(list.len(), 1, "没地址的条目直接丢");
        assert_eq!(list[0].name, "10.7.1.1", "名字空就用地址顶上");
        let bom = "\u{feff}{\"targets\":[{\"host\":\"10.7.2.2\"}]}";
        assert_eq!(parse_targets(bom).len(), 1, "带 BOM 的 json 也要认");
    }

    #[test]
    fn view_puts_selected_peer_first_and_bumps_revision() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        upsert("", "视图目标", "10.66.0.6", 4321, "phone", true);
        let picked = snapshot()
            .into_iter()
            .find(|p| p.host == "10.66.0.6")
            .expect("刚登记的设备应该在列表里");
        toggle_target(&picked);
        let before = revision();
        let v = view();
        assert_eq!(v.peers[0].host, "10.66.0.6");
        assert_eq!(
            v.selected.iter().map(|p| p.host.as_str()).collect::<Vec<_>>(),
            vec!["10.66.0.6"]
        );
        assert!(v.revision >= before);
        reset();
    }
}
