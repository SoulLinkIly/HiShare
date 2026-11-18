use std::cell::RefCell;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[cfg(windows)]
use crate::card;

#[cfg(not(windows))]
mod card {
    pub fn set_outgoing(_ours: bool) {}
    pub fn set_batch(_files: usize) {}
    pub fn start(_name: &str) {}
    pub fn show_progress(_name: &str) {}
    pub fn rename(_name: &str) {}
    pub fn waiting(_name: &str) {}
    pub fn preparing(_name: &str) {}
    pub fn update(_received: u64, _total: u64) {}
    pub fn set_views(_views: Vec<(String, u64, u64, u32)>) {}
    pub fn set_media(_v: bool) {}
    pub fn is_media_name(_name: &str) -> bool {
        false
    }
    pub fn finish() {}

fn fail() {}
    pub fn abort() {}
    pub fn state() -> u8 {
        0
    }
}

pub const PREPARING: u8 = 0;
pub const WAITING: u8 = 1;
pub const RUNNING: u8 = 2;
pub const DONE: u8 = 3;
pub const FAILED: u8 = 4;

pub struct Task {
    outgoing: AtomicU8,
    label: Mutex<String>,

    target: Mutex<String>,

    files: AtomicUsize,
    written: AtomicU64,
    total: AtomicU64,

    speed_kb: AtomicU32,
    last_ms: AtomicU32,
    last_rx: AtomicU64,
    state: AtomicU8,
    rendered: AtomicU8,
}

impl Task {
    fn new(outgoing: bool) -> Self {
        Self {
            outgoing: AtomicU8::new(u8::from(outgoing)),
            label: Mutex::new(String::new()),
            target: Mutex::new(String::new()),
            files: AtomicUsize::new(0),
            written: AtomicU64::new(0),
            total: AtomicU64::new(0),
            speed_kb: AtomicU32::new(0),
            last_ms: AtomicU32::new(crate::clock::now_millis() as u32),
            last_rx: AtomicU64::new(0),
            state: AtomicU8::new(PREPARING),
            rendered: AtomicU8::new(u8::MAX),
        }
    }

    fn label(&self) -> String {
        self.label.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn target(&self) -> String {
        self.target.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn set_label(&self, text: &str) {
        if let Ok(mut slot) = self.label.lock() {
            *slot = text.to_string();
        }
    }

    fn alive(&self) -> bool {
        self.state.load(Ordering::SeqCst) < DONE
    }
}

static TASKS: Mutex<Vec<Arc<Task>>> = Mutex::new(Vec::new());

thread_local! {
    static CURRENT: RefCell<Option<Arc<Task>>> = const { RefCell::new(None) };

    static PENDING_TARGET: RefCell<Option<String>> = const { RefCell::new(None) };
}

pub fn set_target(name: &str) {
    PENDING_TARGET.with(|slot| *slot.borrow_mut() = Some(name.to_string()));
}

fn target_name(t: &Task) -> String {
    let name = t.target();
    if name.is_empty() {
        "传输中".to_string()
    } else {
        name
    }
}

pub fn begin(outgoing: bool, label: &str, files: usize, total: u64) -> Arc<Task> {
    let task = Arc::new(Task::new(outgoing));
    task.set_label(label);
    if let Some(name) = PENDING_TARGET.with(|slot| slot.borrow().clone()) {
        if let Ok(mut slot) = task.target.lock() {
            *slot = name;
        }
    }
    task.files.store(files, Ordering::SeqCst);
    task.total.store(total, Ordering::SeqCst);
    if let Ok(mut list) = TASKS.lock() {
        list.retain(|t| t.alive());
        list.push(task.clone());
    }
    bind(task.clone());
    task
}

pub struct Guard(Arc<Task>);

impl Guard {
    pub fn new(task: Arc<Task>) -> Self {
        Self(task)
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.0.alive() {
            return;
        }
        bind(self.0.clone());
        self.0.state.store(DONE, Ordering::SeqCst);
        if live().is_empty() {
            if let Ok(mut list) = TASKS.lock() {
                list.clear();
            }
        }
        unbind();
    }
}

pub fn bind(task: Arc<Task>) {
    CURRENT.with(|slot| {
        *slot.borrow_mut() = Some(task);
    });
}

pub fn unbind() {
    CURRENT.with(|slot| {
        *slot.borrow_mut() = None;
    });
}

fn current() -> Option<Arc<Task>> {
    CURRENT.with(|slot| slot.borrow().clone())
}

fn live() -> Vec<Arc<Task>> {
    let Ok(list) = TASKS.lock() else {
        return Vec::new();
    };
    list.iter().filter(|t| t.alive()).cloned().collect()
}

pub fn active() -> usize {
    live().len()
}

fn summary(tasks: &[Arc<Task>]) -> (String, u64, u64, usize, bool) {
    let n = tasks.len();
    if n == 1 {
        let t = &tasks[0];
        return (
            t.label(),
            t.written.load(Ordering::SeqCst),
            t.total.load(Ordering::SeqCst),
            t.files.load(Ordering::SeqCst),
            t.outgoing.load(Ordering::SeqCst) == 1,
        );
    }
    let mut written = 0u64;
    let mut total = 0u64;
    let mut outgoing = false;
    for t in tasks {
        written += t.written.load(Ordering::SeqCst);
        total += t.total.load(Ordering::SeqCst);
        outgoing |= t.outgoing.load(Ordering::SeqCst) == 1;
    }

    (format!("{n} 个任务"), written, total, 0, outgoing)
}

fn render() {
    let tasks = live();
    if tasks.is_empty() {
        return;
    }

    let (_, _, _, files, outgoing) = summary(&tasks);
    card::set_outgoing(outgoing);

    let views: Vec<(String, u64, u64, u32)> = tasks
        .iter()
        .map(|t| {
            (
                target_name(t),
                t.written.load(Ordering::SeqCst),
                t.total.load(Ordering::SeqCst),
                t.speed_kb.load(Ordering::SeqCst),
            )
        })
        .collect();

    if tasks.len() == 1 {
        let t = &tasks[0];
        let state = t.state.load(Ordering::SeqCst);
        let previous = t.rendered.swap(state, Ordering::SeqCst);
        let name = &views[0].0;
        if previous != state {
            match state {
                PREPARING => card::preparing(name),
                WAITING => card::waiting(name),
                RUNNING => card::start(name),
                _ => {}
            }
        } else if state == RUNNING {
            card::rename(name);
        }
        if state == RUNNING {
            card::update(views[0].1, views[0].2);
        }
    } else {

        card::show_progress(&views[0].0);
    }

    let media = tasks.len() == 1 && files <= 1 && card::is_media_name(&tasks[0].label());
    card::set_media(media);

    card::set_views(views);
}

fn enter(state: u8, name: &str) {
    let Some(t) = current() else {
        match state {
            PREPARING => card::preparing(name),
            WAITING => card::waiting(name),
            _ => card::start(name),
        }
        return;
    };
    t.set_label(name);
    t.state.store(state, Ordering::SeqCst);
    render();
}

pub fn set_outgoing(ours: bool) {
    match current() {
        Some(t) => {
            t.outgoing.store(u8::from(ours), Ordering::SeqCst);
            render();
        }
        None => card::set_outgoing(ours),
    }
}

pub fn set_batch(files: usize) {
    if let Some(t) = current() {
        t.files.store(files, Ordering::SeqCst);
        render();
    }
}

pub fn preparing(name: &str) {
    enter(PREPARING, name);
}

pub fn waiting(name: &str) {
    enter(WAITING, name);
}

pub fn start(name: &str) {
    enter(RUNNING, name);
}

pub fn update(written: u64, total: u64) {
    match current() {
        Some(t) => {
            t.written.store(written, Ordering::SeqCst);
            if total > 0 {
                t.total.store(total, Ordering::SeqCst);
            }
            if t.state.load(Ordering::SeqCst) != RUNNING {
                t.state.store(RUNNING, Ordering::SeqCst);
            }

            let now = crate::clock::now_millis() as u32;
            let dt = now.wrapping_sub(t.last_ms.load(Ordering::SeqCst));
            if dt >= 500 {
                let db = written.saturating_sub(t.last_rx.load(Ordering::SeqCst));
                let kb = (db / dt.max(1) as u64).min(u32::MAX as u64) as u32;
                t.speed_kb.store(kb, Ordering::SeqCst);
                t.last_ms.store(now, Ordering::SeqCst);
                t.last_rx.store(written, Ordering::SeqCst);
            }
            render();
        }
        None => card::update(written, total),
    }
}

fn retire(task: &Arc<Task>, state: u8) {
    task.state.store(state, Ordering::SeqCst);
}

pub fn finish() {
    let Some(t) = current() else {
        card::finish();
        return;
    };
    retire(&t, DONE);
    if live().is_empty() {
        card::finish();
        if let Ok(mut list) = TASKS.lock() {
            list.clear();
        }
    } else {
        render();
    }
}

pub fn fail() {
    let Some(t) = current() else {
        card::fail();
        return;
    };
    retire(&t, FAILED);
    if live().is_empty() {
        card::fail();
        if let Ok(mut list) = TASKS.lock() {
            list.clear();
        }
    } else {
        render();
    }
}

pub fn abort() {
    match current() {
        Some(t) => {
            if t.state.load(Ordering::SeqCst) < DONE {
                fail();
            }
        }
        None => card::abort(),
    }
}

#[cfg(test)]
pub fn clear_for_test() {
    if let Ok(mut list) = TASKS.lock() {
        list.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static SERIAL: Mutex<()> = Mutex::new(());

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        clear_for_test();
        guard
    }

    #[test]
    fn two_tasks_aggregate() {
        let _g = serial();
        let a = begin(true, "手机", 3, 100);
        let b = begin(true, "iPad", 5, 200);
        a.written.store(40, Ordering::SeqCst);
        b.written.store(60, Ordering::SeqCst);
        let (label, written, total, files, outgoing) = summary(&live());
        assert_eq!(label, "2 个任务");
        assert_eq!(written, 100);
        assert_eq!(total, 300);
        assert_eq!(files, 0, "多任务不给「本批文件数」：加起来没有意义");
        assert!(outgoing);
        active();
        finish();
        finish();
    }

    #[test]
    fn single_task_shows_real_name() {
        let _g = serial();
        let t = begin(false, "IMG_0001.jpg", 0, 1024);
        t.written.store(512, Ordering::SeqCst);
        let (label, written, _total, _files, outgoing) = summary(&live());
        assert_eq!(label, "IMG_0001.jpg");
        assert_eq!(written, 512);
        assert!(!outgoing);
        finish();
    }

    #[test]
    fn finishing_one_keeps_other_alive() {
        let _g = serial();
        let a = begin(true, "a", 1, 10);
        let b = begin(true, "b", 1, 10);
        bind(a.clone());
        finish();
        assert!(!a.alive(), "a 已结束");
        assert_eq!(live().len(), 1, "只剩 b");
        bind(b.clone());
        finish();
        assert!(live().is_empty(), "都结束就清空");
    }
}
