use std::path::PathBuf;
use std::sync::Mutex;

use crate::peers::PeerInfo;

const MAX_PARALLEL: usize = 4;
const PENDING_MAX: usize = 4;

static ACTIVE: Mutex<Vec<String>> = Mutex::new(Vec::new());
static PENDING: Mutex<Vec<(PeerInfo, Vec<PathBuf>, String)>> = Mutex::new(Vec::new());

pub fn active_count() -> usize {
    ACTIVE.lock().map(|l| l.len()).unwrap_or(0)
}

fn note(text: impl Into<String>) {
    crate::report::log(&text.into());
}

#[cfg(windows)]
fn toast(title: &str, body: &str) {
    crate::tray::notify(title, body);
}

#[cfg(not(windows))]
fn toast(_title: &str, _body: &str) {}

struct Slot {
    key: String,
}

impl Slot {
    fn acquire(peer: &PeerInfo) -> Option<Self> {
        let key = peer.host.clone();
        let mut list = ACTIVE.lock().ok()?;
        if list.iter().any(|k| *k == key) {
            return None;
        }
        if list.len() >= MAX_PARALLEL {
            return None;
        }
        list.push(key.clone());
        Some(Self { key })
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        if let Ok(mut list) = ACTIVE.lock() {
            list.retain(|k| *k != self.key);
        }
    }
}

pub fn send_to(peer: PeerInfo, paths: Vec<PathBuf>, display_name: String) -> Result<usize, String> {
    start(peer, paths, display_name, true)
}

fn start(
    peer: PeerInfo,
    paths: Vec<PathBuf>,
    display_name: String,
    allow_queue: bool,
) -> Result<usize, String> {
    if paths.is_empty() {
        return Err("没有选中文件".to_string());
    }
    let count = paths.len();
    let Some(slot) = Slot::acquire(&peer) else {
        if !allow_queue {
            return Err("目标正忙".to_string());
        }
        let queued = match PENDING.lock() {
            Ok(mut q) if q.len() < PENDING_MAX => {
                q.push((peer, paths, display_name));
                Ok(q.len())
            }
            Ok(_) => Err("排队太多了（最多 4 个），等前面发完再拖"),
            Err(_) => Err("排队的内部状态出错，请稍后重试"),
        };
        return match queued {
            Ok(n) => {
                note(format!(
                    "已排队 {count} 个文件（同一目标在发 / 前面还有 {} 个任务）",
                    n - 1
                ));

                toast(
                    "已排队",
                    &format!(
                        "{count} 个文件在等前一批发完（前面还有 {} 个）",
                        n - 1
                    ),
                );
                Ok(count)
            }
            Err(e) => Err(e.to_string()),
        };
    };

    note(format!(
        "正在发送 {count} 个文件 → {}（并发 {}）",
        peer.name,
        active_count()
    ));

    let spawned = std::thread::Builder::new()
        .name("hishare-send".to_string())
        .spawn(move || {
            let _slot = slot;

            let total: u64 = paths
                .iter()
                .filter_map(|p| std::fs::metadata(p).ok())
                .map(|m| m.len())
                .sum();
            note(format!(
                "正在发送 {count} 个文件 · {:.1} MB → {}",
                total as f64 / 1048576.0,
                peer.name
            ));

            #[cfg(windows)]
            let log = |line: &str| {
                crate::tray::log_line(line);

                let brief: String = line.chars().take(48).collect();
                note(brief);
            };
            #[cfg(not(windows))]
            let log = |_line: &str| {};

            crate::progress::set_target(&peer.name);
            let outcome =
                crate::outbound::transmit(&peer.host, peer.port, &paths, &display_name, &log);
            match outcome {
                Ok(report) => {
                    crate::peers::mark_verdict(&peer.host, crate::peers::Verdict::Open);

                    let message = format!(
                        "已发送 {} 个文件 · {} → {}",
                        report.files.len(),
                        crate::human::bytes(report.bytes),
                        peer.name
                    );
                    note(message.clone());
                    toast("发送完成", &message);
                }
                Err(e) => {
                    crate::tray::log_line(&format!("发送失败（{}）：{e}", peer.name));
                    let mut message = format!("{}：{}", peer.name, e.user_message());
                    if e.kind() == crate::ErrorKind::Rejected {
                        crate::peers::mark_verdict(
                            &peer.host,
                            crate::peers::Verdict::Closed,
                        );
                        if peer.kind != "computer" {
                            message.push_str(
                                "（对方没打开华为分享，或控制中心里没设成\"所有人可见\"）",
                            );
                        }
                    }
                    note(message.clone());
                    toast(e.kind().title(), &message);
                }
            }

            drop(_slot);
            drain_pending();
        });

    if let Err(e) = spawned {
        note(format!("无法创建发送线程：{e}"));
        return Err(format!("无法创建发送线程：{e}"));
    }

    Ok(count)
}

fn drain_pending() {
    loop {
        if active_count() >= MAX_PARALLEL {
            break;
        }
        let next = match PENDING.lock() {
            Ok(mut q) if !q.is_empty() => Some(q.remove(0)),
            _ => None,
        };
        let Some((peer, paths, display)) = next else {
            break;
        };
        crate::tray::log_line(&format!(
            "[排队] 开始发送之前排队的 {} 个文件 → {}",
            paths.len(),
            peer.name
        ));

        let backup = (peer.clone(), paths.clone(), display.clone());
        match start(peer, paths, display, false) {
            Ok(_) => continue,
            Err(e) => {
                if let Ok(mut q) = PENDING.lock() {
                    q.insert(0, backup);
                }
                crate::tray::log_line(&format!("[排队] 这一轮起不来（{e}），任务留在队列里"));
                break;
            }
        }
    }
}

pub fn send_many(
    targets: Vec<PeerInfo>,
    paths: Vec<PathBuf>,
    display_name: String,
) -> Result<usize, String> {
    if paths.is_empty() {
        return Err("没有选中文件".to_string());
    }
    let count = paths.len();
    let total = targets.len();
    let mut started = 0usize;
    let mut first_error: Option<String> = None;
    for peer in targets {
        match send_to(peer, paths.clone(), display_name.clone()) {
            Ok(_) => started += 1,
            Err(e) => {
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
        }
    }
    if started == 0 {
        return Err(first_error.unwrap_or_else(|| "没有可发送的目标".to_string()));
    }
    note(format!(
        "正在同时发送 {count} 个文件 → {started}/{total} 台设备"
    ));
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    static SLOT_LOCK: Mutex<()> = Mutex::new(());

    fn lock_slots() -> std::sync::MutexGuard<'static, ()> {
        SLOT_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn peer(name: &str, host: &str) -> PeerInfo {
        PeerInfo {
            id: String::new(),
            name: name.to_string(),
            host: host.to_string(),
            port: 1,
            kind: "phone".to_string(),
            seen: std::time::Instant::now(),
            gap: std::time::Duration::ZERO,
            accepts: true,
        }
    }

    #[test]
    fn same_peer_cannot_run_twice_but_other_peer_can() {
        let _guard = lock_slots();
        let a = peer("A", "10.0.0.1");
        let first = Slot::acquire(&a).expect("第一路应拿到槽位");
        assert!(Slot::acquire(&a).is_none(), "同一台设备不能并发两路");
        let b = peer("B", "10.0.0.2");
        let second = Slot::acquire(&b).expect("另一台设备应能并发");
        assert_eq!(active_count(), 2);
        drop(first);
        assert_eq!(active_count(), 1);
        drop(second);
        assert_eq!(active_count(), 0);
    }

    #[test]
    fn parallel_limit_is_enforced() {
        let _guard = lock_slots();
        let mut held = Vec::new();
        for i in 0..MAX_PARALLEL {
            let p = peer("x", &format!("10.1.0.{i}"));
            held.push(Slot::acquire(&p).expect("未到上限都应成功"));
        }
        let extra = peer("y", "10.1.0.99");
        assert!(Slot::acquire(&extra).is_none(), "超过上限必须拒绝");
        held.clear();
        assert_eq!(active_count(), 0);
    }

    #[test]
    fn send_many_rejects_empty_file_list() {
        let _guard = lock_slots();
        let err = send_many(vec![peer("只有一台", "10.0.0.1")], Vec::new(), "本机".into());
        assert_eq!(err, Err("没有选中文件".to_string()), "空文件列表不该起线程");
    }
}
