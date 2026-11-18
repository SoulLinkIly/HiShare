#![cfg(windows)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    DeleteObject, DrawTextW, GetMonitorInfoW, MonitorFromPoint, SelectObject, SetBkMode,
    SetTextColor, DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, MONITORINFO,
    MONITOR_DEFAULTTOPRIMARY, TRANSPARENT,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TRACKMOUSEEVENT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics,
    GetWindowLongPtrW, PeekMessageW, PostThreadMessageW, SetTimer, SetWindowLongPtrW, SetWindowPos,
    ShowWindow, TranslateMessage, MSG, SWP_NOACTIVATE, SW_SHOWNOACTIVATE,
};

use crate::layered::Canvas;
use crate::ui::draw as ui_draw;
use crate::ui::font::{ui as ui_font, WEIGHT_NORMAL};
use crate::ui::popup;
use crate::ui::theme;

const CLASS: &str = "HiShareToast";

const CARD_W: i32 = 364;
const CARD_H: i32 = 84;

const SHADOW_PAD: i32 = 14;
const MARGIN: i32 = 20;
const GAP: i32 = 12;
const RADIUS_LOGICAL: i32 = 14;

const BAR_W: i32 = 3;

const BAR_INSET: i32 = 19;

const CARD_BG: u32 = 0x00FF_FFFF;
const CARD_BG_HOVER: u32 = 0x00FD_FA_FB;
const CARD_BORDER: u32 = 0x00EC_E2_E7;

const TEXT_TITLE: u32 = 0x001A_10_14;

const TEXT_BODY: u32 = 0x004B_40_45;

const BAR_COLOR: u32 = theme::ACCENT;

const FONT_TITLE: i32 = 16;
const FONT_BODY: i32 = 15;

const SHADOW_ALPHA: u8 = 34;

const SHADOW_OFFSET_Y: i32 = 2;

const MAX_ON_SCREEN: usize = 3;

const PENDING_MAX: usize = 16;

const TTL: Duration = Duration::from_millis(5200);
const FADE_IN: Duration = Duration::from_millis(140);
const FADE_OUT: Duration = Duration::from_millis(200);

const MIN_AFTER_HOVER: Duration = Duration::from_millis(400);
const TICK_MS: u32 = 30;

const EX_STYLE: u32 = 0x0000_0008
    | 0x0008_0000
    | 0x0000_0080
    | 0x0800_0000;

const WM_ERASEBKGND: u32 = 0x0014;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_TIMER: u32 = 0x0113;
const WM_DESTROY: u32 = 0x0002;

const WM_TOAST_WAKE: u32 = 0x8000 + 0x31;

const GWLP_USERDATA: i32 = -21;
const TME_LEAVE: u32 = 0x0000_0002;
const SWP_NOZORDER: u32 = 0x0004;
const HWND_TOPMOST: HWND = -1isize as HWND;

struct Item {
    title: String,
    body: String,
}

impl Item {
    fn same(&self, other: &Item) -> bool {
        self.title == other.title && self.body == other.body
    }
}

#[derive(Default)]
struct Pending {
    items: VecDeque<Item>,
}

impl Pending {

    fn push(&mut self, item: Item) -> bool {
        if self.items.iter().any(|i| i.same(&item)) {
            return false;
        }
        if self.items.len() >= PENDING_MAX {
            self.items.pop_front();
        }
        self.items.push_back(item);
        true
    }

    fn pop(&mut self) -> Option<Item> {
        self.items.pop_front()
    }
}

static PENDING: Mutex<Pending> = Mutex::new(Pending { items: VecDeque::new() });

static THREAD: AtomicU32 = AtomicU32::new(0);
static BOOTED: AtomicBool = AtomicBool::new(false);

static BROKEN: AtomicBool = AtomicBool::new(false);

static SHOWING: Mutex<Vec<isize>> = Mutex::new(Vec::new());

pub fn push(title: &str, body: &str) -> bool {
    if !enabled() || BROKEN.load(Ordering::SeqCst) {
        return false;
    }
    if !boot() {
        return false;
    }
    if let Ok(mut q) = PENDING.lock() {
        q.push(Item { title: title.to_string(), body: body.to_string() });
    }
    let tid = THREAD.load(Ordering::SeqCst);
    if tid == 0 {
        return false;
    }
    unsafe { PostThreadMessageW(tid, WM_TOAST_WAKE, 0, 0) };
    true
}

fn enabled() -> bool {
    !matches!(crate::env::read("HISHARE_TOAST"), Some(v) if v.trim() == "0")
}

fn boot() -> bool {
    if BOOTED.swap(true, Ordering::SeqCst) {

        for _ in 0..200 {
            if THREAD.load(Ordering::SeqCst) != 0 {
                return true;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        return false;
    }

    let (tx, rx) = std::sync::mpsc::channel::<bool>();
    let spawned = std::thread::Builder::new()
        .name("hishare-toast".to_string())
        .spawn(move || {
            if !run(tx) {
                BROKEN.store(true, Ordering::SeqCst);
            }
        });
    if spawned.is_err() {
        BROKEN.store(true, Ordering::SeqCst);
        return false;
    }
    rx.recv_timeout(Duration::from_millis(300)).unwrap_or(false)
}

fn run(ready: std::sync::mpsc::Sender<bool>) -> bool {
    crate::tray::enable_dpi_awareness();
    popup::register_class(CLASS, Some(wndproc), 0);

    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, 0);
    }
    THREAD.store(unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() }, Ordering::SeqCst);
    let _ = ready.send(true);
    unsafe { SetTimer(std::ptr::null_mut(), 0, TICK_MS, None) };

    loop {
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        let got = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
        if got <= 0 {
            break;
        }
        match msg.message {
            WM_TOAST_WAKE => tick(),
            WM_TIMER => tick(),
            _ => unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            },
        }
    }
    true
}

fn tick() {
    let now = Instant::now();
    let work = work_area();

    let snapshot: Vec<isize> = SHOWING.lock().map(|l| l.clone()).unwrap_or_default();
    for (index, &raw) in snapshot.iter().enumerate() {
        let hwnd = raw as HWND;
        let state = state_of(hwnd);
        if state.is_null() {
            continue;
        }
        let st = unsafe { &mut *state };

        let (x, y) = slot_rect(work, st.w, st.h, index, st.dpi);
        if x != st.x || y != st.y {
            st.x = x;
            st.y = y;
            unsafe {
                SetWindowPos(hwnd, std::ptr::null_mut(), x, y, st.w, st.h, SWP_NOZORDER | SWP_NOACTIVATE);
            }
        }

        if st.hovered {
            if let Some(f) = st.frozen {
                st.deadline = now + f;
            }
        }
        if st.closing.is_none() && !st.hovered && now >= st.deadline {
            st.closing = Some(now);
        }
        let alpha = st.alpha(now);

        if st.closing.is_some() && alpha == 0 {
            unsafe { DestroyWindow(hwnd) };
            continue;
        }

        let mut push = false;
        if st.dirty {
            render(st, now);
            st.dirty = false;
            push = true;
        }

        let ratio = bar_ratio(st.remaining(now));
        let bar_px = (bar_span(st.dpi, st.h) as f32 * ratio).round() as i32;
        if bar_px != st.bar_px {
            if let Some(canvas) = st.canvas.as_mut() {
                unsafe { paint_bar(canvas, st.dpi, st.h, st.hovered, ratio) };
            }
            st.bar_px = bar_px;
            push = true;
        }
        if st.last_alpha != alpha {
            push = true;
        }
        if push {
            if let Some(canvas) = st.canvas.as_mut() {
                let radius = theme::px(RADIUS_LOGICAL, st.dpi);
                unsafe { canvas.commit(hwnd, st.x, st.y, radius, alpha) };
                st.last_alpha = alpha;
            }
        }
    }

    let on_screen = SHOWING.lock().map(|l| l.len()).unwrap_or(0);
    for slot in on_screen..MAX_ON_SCREEN {
        let item = PENDING.lock().ok().and_then(|mut q| q.pop());
        let Some(item) = item else { break };
        show(item, slot);
    }
}

fn show(item: Item, index: usize) {
    let dpi = popup::system_dpi();
    let win_w = CARD_W + SHADOW_PAD * 2;
    let win_h = CARD_H + SHADOW_PAD * 2;
    let w = theme::px(win_w, dpi);
    let h = theme::px(win_h, dpi);
    let (x, y) = slot_rect(work_area(), w, h, index, dpi);

    let Some(win) = popup::create_layered(
        CLASS,
        "HiShare",
        (win_w, win_h),
        EX_STYLE,
        popup::Placement::At(x, y),
    ) else {

        BROKEN.store(true, Ordering::SeqCst);
        crate::report::log("[提示] 自绘提示窗创建失败，已退回系统气泡");
        return;
    };

    let st_title = item.title.clone();
    let font_title = unsafe { ui_font(theme::px(FONT_TITLE, dpi), WEIGHT_NORMAL) };
    let font_body = unsafe { ui_font(theme::px(FONT_BODY, dpi), WEIGHT_NORMAL) };
    let now = Instant::now();
    let mut canvas = unsafe { Canvas::new(win.w, win.h) };
    if let Some(c) = canvas.as_mut() {
        c.pad = theme::px(SHADOW_PAD, dpi);
        c.manual_alpha = true;
    }
    let mut st = Box::new(CardState {
        canvas,
        title: item.title,
        body: item.body,
        dpi,
        w: win.w,
        h: win.h,
        x: win.x,
        y: win.y,
        born: now,
        deadline: now + TTL,
        closing: None,
        hovered: false,
        dirty: true,
        last_alpha: 0,
        bar_px: -1,
        frozen: None,
        font_title: font_title as isize,
        font_body: font_body as isize,
    });
    st.apply_real_dpi(win.hwnd);

    unsafe {
        SetWindowLongPtrW(win.hwnd, GWLP_USERDATA, Box::into_raw(st) as isize);
        ShowWindow(win.hwnd, SW_SHOWNOACTIVATE);
    }
    crate::trace::note("toast", &format!("显示提示：{}", st_title));
    if let Ok(mut list) = SHOWING.lock() {
        list.push(win.hwnd as isize);
    }
    tick();
}

fn state_of(hwnd: HWND) -> *mut CardState {
    unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut CardState }
}

struct CardState {
    canvas: Option<Canvas>,
    title: String,
    body: String,
    dpi: i32,
    w: i32,
    h: i32,
    x: i32,
    y: i32,
    born: Instant,
    deadline: Instant,
    closing: Option<Instant>,
    hovered: bool,
    dirty: bool,

    last_alpha: u8,

    bar_px: i32,

    frozen: Option<Duration>,
    font_title: isize,
    font_body: isize,
}

impl CardState {

    fn apply_real_dpi(&mut self, hwnd: HWND) {
        let real = unsafe { GetDpiForWindow(hwnd) } as i32;
        if real <= 0 || real == self.dpi {
            return;
        }
        let w = theme::px(CARD_W + SHADOW_PAD * 2, real);
        let h = theme::px(CARD_H + SHADOW_PAD * 2, real);
        self.dpi = real;
        self.w = w;
        self.h = h;
        self.canvas = unsafe { Canvas::new(w, h) };
        if let Some(c) = self.canvas.as_mut() {
            c.pad = theme::px(SHADOW_PAD, real);
            c.manual_alpha = true;
        }
        self.dirty = true;
        self.last_alpha = 0;

        unsafe {
            for f in [self.font_title, self.font_body] {
                if f != 0 {
                    DeleteObject(f as _);
                }
            }
        }
        self.font_title = unsafe { ui_font(theme::px(FONT_TITLE, real), WEIGHT_NORMAL) } as isize;
        self.font_body = unsafe { ui_font(theme::px(FONT_BODY, real), WEIGHT_NORMAL) } as isize;
        let (x, y) = slot_rect(work_area(), w, h, 0, real);
        self.x = x;
        self.y = y;
        unsafe {
            SetWindowPos(hwnd, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE);
        }
    }

    fn remaining(&self, now: Instant) -> Duration {
        if self.closing.is_some() {
            return Duration::ZERO;
        }
        self.deadline.saturating_duration_since(now)
    }

    fn alpha(&self, now: Instant) -> u8 {        if let Some(start) = self.closing {
            let e = now.saturating_duration_since(start).as_millis();
            let total = FADE_OUT.as_millis().max(1);
            if e >= total {
                return 0;
            }
            return (255 * (total - e) / total) as u8;
        }
        let e = now.saturating_duration_since(self.born).as_millis();
        let total = FADE_IN.as_millis().max(1);
        if e < total {
            (255 * e / total) as u8
        } else {
            255
        }
    }
}

fn bar_ratio(remain: Duration) -> f32 {
    let total = TTL.as_millis().max(1) as f32;
    (remain.as_millis() as f32 / total).clamp(0.0, 1.0)
}

fn bar_span(dpi: i32, h_win: i32) -> i32 {
    let pad = theme::px(SHADOW_PAD, dpi);
    let inset = theme::px(BAR_INSET, dpi);
    ((h_win - 2 * pad) - 2 * inset).max(0)
}

unsafe fn paint_bar(canvas: &mut Canvas, dpi: i32, h_win: i32, hovered: bool, ratio: f32) {
    let hdc = canvas.mem;
    let px = |v: i32| theme::px(v, dpi);
    let pad = px(SHADOW_PAD);
    let x = pad + px(14);
    let w = px(BAR_W);
    let top = pad + px(BAR_INSET);
    let bottom = h_win - pad - px(BAR_INSET);
    let bg = if hovered { CARD_BG_HOVER } else { CARD_BG };

    ui_draw::fill(hdc, x, top, x + w, bottom, bg);
    let h = ((bottom - top) as f32 * ratio).round() as i32;
    if h > 0 {

        ui_draw::fill(hdc, x, bottom - h, x + w, bottom, BAR_COLOR);
    }

    canvas.set_alpha_rect(x, top, w, bottom - top, 255);
}

fn render(st: &mut CardState, now: Instant) {

    let hovered = st.hovered;
    let ratio = bar_ratio(st.remaining(now));
    let bar_px = (bar_span(st.dpi, st.h) as f32 * ratio).round() as i32;
    let Some(canvas) = st.canvas.as_mut() else { return };
    let (w, h) = (canvas.w, canvas.h);
    let hdc = canvas.mem;
    let dpi = st.dpi;
    let px = |v: i32| theme::px(v, dpi);

    let pad = px(SHADOW_PAD);
    let (x1, y1, x2, y2) = (pad, pad, w - pad, h - pad);
    let radius = px(RADIUS_LOGICAL);

    unsafe {

        ui_draw::fill(hdc, 0, 0, w, h, 0x0000_0000);

        let bg = if hovered { CARD_BG_HOVER } else { CARD_BG };

        ui_draw::fill(hdc, x1, y1, x2, y2, bg);

        paint_bar(canvas, dpi, h, hovered, ratio);

        ui_draw::round_stroke(hdc, x1, y1, x2, y2, radius * 2, CARD_BORDER, 1);

        SetBkMode(hdc, TRANSPARENT as i32);

        let text_left = x1 + px(28);
        let text_right = x2 - px(20);

        SelectObject(hdc, st.font_title as _);
        SetTextColor(hdc, TEXT_TITLE);
        let mut r0 = RECT { left: text_left, top: y1 + px(12), right: text_right, bottom: y1 + px(38) };
        let u0: Vec<u16> = st.title.encode_utf16().collect();
        DrawTextW(hdc, u0.as_ptr(), u0.len() as i32, &mut r0, DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX);

        SelectObject(hdc, st.font_body as _);
        SetTextColor(hdc, TEXT_BODY);

        let mut r1 = RECT { left: text_left, top: y1 + px(39), right: text_right, bottom: y2 - px(11) };
        let u1: Vec<u16> = st.body.encode_utf16().collect();
        DrawTextW(hdc, u1.as_ptr(), u1.len() as i32, &mut r1, DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX);

        canvas.reset_alpha();
        canvas.paint_shadow(radius, px(SHADOW_OFFSET_Y), SHADOW_ALPHA);

        canvas.freeze();
    }
    st.bar_px = bar_px;
}

fn slot_rect(work: (i32, i32, i32, i32), w: i32, h: i32, index: usize, dpi: i32) -> (i32, i32) {
    let (left, top, right, bottom) = work;
    let pad = theme::px(SHADOW_PAD, dpi);
    let margin = theme::px(MARGIN, dpi);
    let gap = theme::px(GAP, dpi);
    let card_h = h - 2 * pad;
    let x = right - w + pad - margin;
    let y = bottom - h + pad - margin - index as i32 * (card_h + gap);
    (x.max(left), y.max(top))
}

fn work_area() -> (i32, i32, i32, i32) {
    unsafe {
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTOPRIMARY);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(mon, &mut info) != 0 {
            let r = info.rcWork;
            return (r.left, r.top, r.right, r.bottom);
        }

        (0, 0, GetSystemMetrics(0), GetSystemMetrics(1))
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            let state = state_of(hwnd);
            if !state.is_null() {
                let st = &mut *state;
                if !st.hovered {
                    st.hovered = true;
                    st.dirty = true;

                    st.frozen = Some(st.deadline.saturating_duration_since(Instant::now()));
                }
                let mut track = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                TrackMouseEvent(&mut track);
            }
            0
        }
        WM_MOUSELEAVE => {
            let state = state_of(hwnd);
            if !state.is_null() {
                let st = &mut *state;
                st.hovered = false;
                st.dirty = true;

                let remain = st.frozen.take().unwrap_or(TTL);
                st.deadline = Instant::now() + remain.max(MIN_AFTER_HOVER);
            }
            0
        }
        WM_LBUTTONDOWN => {

            let state = state_of(hwnd);
            if !state.is_null() {
                let st = &mut *state;
                if st.closing.is_none() {
                    st.closing = Some(Instant::now());
                }
            }
            0
        }
        WM_ERASEBKGND => 1,
        WM_DESTROY => {
            let state = state_of(hwnd);
            if !state.is_null() {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                let st = Box::from_raw(state);
                for f in [st.font_title, st.font_body] {
                    if f != 0 {
                        DeleteObject(f as _);
                    }
                }
            }
            if let Ok(mut list) = SHOWING.lock() {
                list.retain(|h| *h != hwnd as isize);
            }
            0
        }
        _ => windows_sys::Win32::UI::WindowsAndMessaging::DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(t: &str, b: &str) -> Item {
        Item { title: t.to_string(), body: b.to_string() }
    }

    #[test]
    fn same_text_is_dropped_once() {
        let mut q = Pending::default();
        assert!(q.push(item("接收目录已更新", "D:\\收")));
        assert!(!q.push(item("接收目录已更新", "D:\\收")), "连点两次只该飘一条");
        assert!(q.push(item("接收目录已更新", "E:\\收")), "内容不同就是两条");
        assert_eq!(q.items.len(), 2);
    }

    #[test]
    fn queue_drops_the_oldest_beyond_cap() {
        let mut q = Pending::default();
        for i in 0..(PENDING_MAX + 5) {
            q.push(item("t", &format!("第 {i} 条")));
        }
        assert_eq!(q.items.len(), PENDING_MAX, "上限必须收住，否则会攒一堆过期提示");
        assert_eq!(q.items.front().unwrap().body, "第 5 条", "丢的是最旧的");
    }

    #[test]
    fn slots_stack_upward_from_bottom_right() {
        let work = (0, 0, 1920, 1040);

        let (w, h) = (CARD_W + SHADOW_PAD * 2, CARD_H + SHADOW_PAD * 2);
        let (x0, y0) = slot_rect(work, w, h, 0, 96);
        assert_eq!(x0 + SHADOW_PAD + CARD_W, 1920 - MARGIN, "贴的是卡片的边，不是窗口的边");
        assert_eq!(y0 + SHADOW_PAD + CARD_H, 1040 - MARGIN, "同上：投影留白不占边距");

        let (_, y1) = slot_rect(work, w, h, 1, 96);
        assert_eq!(y0 - y1, CARD_H + GAP, "第二条要落在第一条正上方，间距按卡片高度算");

        let (w, h) = (w * 3 / 2, h * 3 / 2);
        let (x, y) = slot_rect(work, w, h, 1, 144);
        assert_eq!(x, 1920 - w + SHADOW_PAD * 3 / 2 - MARGIN * 3 / 2);
        assert_eq!(y, 1040 - h + SHADOW_PAD * 3 / 2 - MARGIN * 3 / 2 - (CARD_H * 3 / 2 + GAP * 3 / 2));
    }

    #[test]
    fn slot_never_leaves_a_short_work_area() {

        let work = (0, 0, 300, 200);
        let (x, y) = slot_rect(work, CARD_W + SHADOW_PAD * 2, CARD_H + SHADOW_PAD * 2, 2, 96);
        assert_eq!((x, y), (0, 0));
    }

    #[test]
    fn bar_ratio_runs_from_full_to_empty() {
        assert_eq!(bar_ratio(TTL), 1.0, "刚出现是满格");
        assert!((bar_ratio(TTL / 2) - 0.5).abs() < 0.01, "过半时约一半");
        assert_eq!(bar_ratio(Duration::ZERO), 0.0, "时间用完条也空了");
        assert_eq!(bar_ratio(TTL * 3), 1.0, "意外多给的时间不能画出超长的条");
    }

    #[test]
    fn fade_in_reaches_full_then_fades_out_to_zero() {
        let now = Instant::now();
        let mk = |born_offset: Duration, closing: Option<Duration>| CardState {
            canvas: None,
            title: String::new(),
            body: String::new(),
            dpi: 96,
            w: CARD_W,
            h: CARD_H,
            x: 0,
            y: 0,
            born: now - born_offset,
            deadline: now,
            closing: closing.map(|d| now - d),
            hovered: false,
            dirty: false,
            last_alpha: 0,
            bar_px: -1,
            frozen: None,
            font_title: 0,
            font_body: 0,
        };
        assert_eq!(mk(Duration::ZERO, None).alpha(now), 0, "刚出现是全透明");
        assert_eq!(mk(FADE_IN, None).alpha(now), 255, "淡入结束是实心");
        assert_eq!(mk(TTL, None).alpha(now), 255, "停留期间不变");
        let half = FADE_OUT / 2;
        let mid = mk(TTL, Some(half)).alpha(now);
        assert!(mid > 0 && mid < 255, "淡出是渐变的，实际 {mid}");
        assert_eq!(mk(TTL, Some(FADE_OUT)).alpha(now), 0, "淡出结束必须归零，否则窗口收不掉");
    }
}
