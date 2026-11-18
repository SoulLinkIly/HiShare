#![cfg(windows)]

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{HWND, POINT, SIZE};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, GetDC, ReleaseDC, SelectObject, BI_RGB, BITMAPINFO,
    BITMAPINFOHEADER, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    UpdateLayeredWindow, UPDATE_LAYERED_WINDOW_FLAGS, ULW_ALPHA,
};

unsafe impl Send for Canvas {}

pub struct Canvas {
    pub mem: HDC,
    pub bits: *mut u8,
    pub w: i32,
    pub h: i32,

    pub pad: i32,

    pub manual_alpha: bool,
    dib: HBITMAP,

    mask: Vec<u8>,

    partial: Vec<u32>,
    mask_radius: i32,
    mask_pad: i32,

    snapshot: Vec<u8>,
}

impl Canvas {
    pub unsafe fn new(w: i32, h: i32) -> Option<Self> {
        if w <= 0 || h <= 0 {
            return None;
        }
        let screen = GetDC(std::ptr::null_mut());
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w;
        bmi.bmiHeader.biHeight = -h;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;

        let mut bits: *mut c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(screen, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        let mem = CreateCompatibleDC(screen);
        ReleaseDC(std::ptr::null_mut(), screen);
        if dib.is_null() || mem.is_null() || bits.is_null() {
            if !mem.is_null() {
                windows_sys::Win32::Graphics::Gdi::DeleteDC(mem);
            }
            if !dib.is_null() {
                windows_sys::Win32::Graphics::Gdi::DeleteObject(dib as _);
            }
            return None;
        }
        SelectObject(mem, dib);
        Some(Canvas {
            mem,
            bits: bits as *mut u8,
            w,
            h,
            pad: 0,
            manual_alpha: false,
            dib,
            mask: Vec::new(),
            partial: Vec::new(),
            mask_radius: -1,
            mask_pad: -1,
            snapshot: Vec::new(),
        })
    }

    pub unsafe fn reset_alpha(&mut self) {
        let n = (self.w * self.h) as usize;
        for i in 0..n {
            *self.bits.add(i * 4 + 3) = 255;
        }
    }

    pub unsafe fn set_alpha_rect(&mut self, x: i32, y: i32, w: i32, h: i32, a: u8) {
        let x0 = x.clamp(0, self.w);
        let y0 = y.clamp(0, self.h);
        let x1 = (x.saturating_add(w)).clamp(0, self.w);
        let y1 = (y.saturating_add(h)).clamp(0, self.h);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let bits = self.bits;
        for yy in y0..y1 {
            let row = (yy * self.w) as usize * 4;
            for xx in x0..x1 {
                *bits.add(row + xx as usize * 4 + 3) = a;
            }
        }
    }

    pub fn freeze(&mut self) {
        let n = (self.w * self.h * 4) as usize;
        self.snapshot.resize(n, 0);
        unsafe { std::ptr::copy_nonoverlapping(self.bits, self.snapshot.as_mut_ptr(), n) };
    }

    pub unsafe fn paint_shadow(&mut self, radius: i32, offset_y: i32, max_alpha: u8) {
        let pad = self.pad;
        if pad <= 0 {
            return;
        }
        let (w, h) = (self.w, self.h);
        let x1 = pad as f32;
        let y1 = pad as f32;
        let x2 = (w - pad) as f32;
        let y2 = (h - pad) as f32;
        let r = radius.max(0) as f32;
        let blur = pad as f32;
        let oy = offset_y as f32;
        let bits = self.bits;
        for y in 0..h {
            let py = y as f32 + 0.5 - oy;
            for x in 0..w {
                if x >= pad && x < w - pad && y >= pad && y < h - pad {
                    continue;
                }
                let px = x as f32 + 0.5;
                let d = round_rect_sdf(px, py, x1, y1, x2, y2, r);
                let a = if d >= blur {
                    0
                } else {
                    let t = 1.0 - d.max(0.0) / blur;
                    (max_alpha as f32 * t * t) as u8
                };
                let base = ((y * w + x) * 4) as usize;
                *bits.add(base) = 0;
                *bits.add(base + 1) = 0;
                *bits.add(base + 2) = 0;
                *bits.add(base + 3) = a;
            }
        }
    }

    fn rebuild_mask(&mut self, radius: i32) {
        let r = radius.max(0);
        let pad = self.pad.max(0);
        if r == self.mask_radius && pad == self.mask_pad && self.mask.len() == (self.w * self.h) as usize {
            return;
        }
        let (w, h) = (self.w, self.h);
        let n = (w * h) as usize;
        self.mask.clear();
        self.mask.resize(n, 255);
        self.partial.clear();
        self.mask_radius = r;
        self.mask_pad = pad;
        if r <= 0 {
            return;
        }
        let cw = w - 2 * pad;
        let ch = h - 2 * pad;
        for y in pad..(h - pad) {
            for x in pad..(w - pad) {
                let a = corner_alpha(x - pad, y - pad, cw, ch, r);
                if a != 255 {
                    let i = (y * w + x) as usize;
                    self.mask[i] = a;
                    self.partial.push(i as u32);
                }
            }
        }
    }

    pub unsafe fn fade_content(&mut self, bg: u32, alpha: u8) {
        if alpha >= 255 {
            return;
        }
        let a = alpha as i32;
        let inv = 255 - a;
        let base = [
            ((bg >> 16) & 0xFF) as i32,
            ((bg >> 8) & 0xFF) as i32,
            (bg & 0xFF) as i32,
        ];
        let n = (self.w * self.h) as usize;
        let bits = self.bits;
        for i in 0..n {
            let p = i * 4;
            for c in 0..3 {
                let v = *bits.add(p + c) as i32;
                *bits.add(p + c) = ((base[c] * inv + v * a + 127) / 255) as u8;
            }
        }
    }

    pub unsafe fn commit(&mut self, hwnd: HWND, x: i32, y: i32, radius: i32, alpha: u8) {
        self.rebuild_mask(radius);

        let n = (self.w * self.h) as usize;
        let bits = self.bits;

        if !self.manual_alpha {
            for i in 0..n {
                *bits.add(i * 4 + 3) = 255;
            }
        }

        if self.snapshot.len() == n * 4 {
            for &pi in &self.partial {
                let base = pi as usize * 4;
                for c in 0..3 {
                    *bits.add(base + c) = self.snapshot[base + c];
                }
            }
        }
        for &pi in &self.partial {
            let i = pi as usize;
            let a = self.mask[i] as u32;
            let base = i * 4;
            for c in 0..3 {
                let v = *bits.add(base + c) as u32;
                *bits.add(base + c) = ((v * a + 127) / 255) as u8;
            }
            *bits.add(base + 3) = a as u8;
        }

        let mut pt_dst = POINT { x, y };
        let size = SIZE { cx: self.w, cy: self.h };
        let pt_src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: 0,
            BlendFlags: 0,

            SourceConstantAlpha: alpha,
            AlphaFormat: 1,
        };
        let screen_dc = GetDC(std::ptr::null_mut());
        UpdateLayeredWindow(
            hwnd,
            screen_dc,
            &mut pt_dst,
            &size,
            self.mem,
            &pt_src,
            0,
            &blend,
            ULW_ALPHA as UPDATE_LAYERED_WINDOW_FLAGS,
        );
        ReleaseDC(std::ptr::null_mut(), screen_dc);
    }

    pub unsafe fn destroy(&mut self) {
        if !self.mem.is_null() {
            windows_sys::Win32::Graphics::Gdi::DeleteDC(self.mem);
            self.mem = std::ptr::null_mut();
        }
        if !self.dib.is_null() {
            windows_sys::Win32::Graphics::Gdi::DeleteObject(self.dib as _);
            self.dib = std::ptr::null_mut();
        }
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe { self.destroy() };
    }
}

pub fn canvas_info(
    slot: Option<&std::sync::Mutex<Option<Canvas>>>,
) -> Option<(HDC, i32, i32)> {
    let slot = slot?;
    let Ok(mut guard) = slot.lock() else {
        return None;
    };
    let canvas = guard.as_mut()?;
    Some((canvas.mem, canvas.w, canvas.h))
}

pub fn install_canvas(
    slot: &std::sync::OnceLock<std::sync::Mutex<Option<Canvas>>>,
    canvas: Option<Canvas>,
) {
    let mut guard = slot
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    *guard = canvas;
}

pub unsafe fn commit_canvas(
    slot: &std::sync::OnceLock<std::sync::Mutex<Option<Canvas>>>,
    hwnd: HWND,
    x: i32,
    y: i32,
    radius: i32,
    alpha: u8,
) {
    let Some(slot) = slot.get() else { return };
    let Ok(mut guard) = slot.lock() else { return };
    let Some(canvas) = guard.as_mut() else { return };
    canvas.commit(hwnd, x, y, radius, alpha);
}

pub fn round_rect_sdf(px: f32, py: f32, x1: f32, y1: f32, x2: f32, y2: f32, r: f32) -> f32 {
    let cx = (x1 + x2) * 0.5;
    let cy = (y1 + y2) * 0.5;
    let hx = ((x2 - x1) * 0.5 - r).max(0.0);
    let hy = ((y2 - y1) * 0.5 - r).max(0.0);
    let qx = (px - cx).abs() - hx;
    let qy = (py - cy).abs() - hy;
    let ox = qx.max(0.0);
    let oy = qy.max(0.0);
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - r
}

pub fn corner_alpha(x: i32, y: i32, w: i32, h: i32, r: i32) -> u8 {    if r <= 0 {
        return 255;
    }
    let (cx, cy) = if x < r && y < r {
        (r as f32, r as f32)
    } else if x >= w - r && y < r {
        ((w - r) as f32, r as f32)
    } else if x < r && y >= h - r {
        (r as f32, (h - r) as f32)
    } else if x >= w - r && y >= h - r {
        ((w - r) as f32, (h - r) as f32)
    } else {
        return 255;
    };
    let dx = x as f32 + 0.5 - cx;
    let dy = y as f32 + 0.5 - cy;
    let d = (dx * dx + dy * dy).sqrt();
    let inner = r as f32 - 0.5;
    let outer = r as f32 + 0.5;
    if d <= inner {
        255
    } else if d >= outer {
        0
    } else {
        (((outer - d) / (outer - inner)) * 255.0) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_fade_out_center_is_opaque() {
        assert_eq!(corner_alpha(0, 0, 100, 50, 10), 0, "最外角完全透明");
        assert_eq!(corner_alpha(50, 25, 100, 50, 10), 255, "正中不透明");
        assert_eq!(corner_alpha(0, 25, 100, 50, 10), 255, "左边中段不透明");
    }

    #[test]
    fn zero_radius_is_a_plain_rectangle() {
        for (x, y) in [(0, 0), (0, 9), (9, 0), (5, 5)] {
            assert_eq!(corner_alpha(x, y, 10, 10, 0), 255);
        }
    }

    #[test]
    fn sdf_matches_the_rect_it_describes() {

        let d = |x: f32, y: f32| round_rect_sdf(x, y, 10.0, 10.0, 110.0, 60.0, 8.0);
        assert!(d(60.0, 35.0) < -20.0, "正中该是「深在内部」");
        assert!(d(5.0, 35.0) > 0.0 && d(5.0, 35.0) < 6.0, "左边 5px 外");
        assert!(d(120.0, 35.0) > 9.0, "右边 10px 外");

        let corner = d(10.0, 10.0);
        assert!((corner - 3.31).abs() < 0.2, "圆角外该比直角更远，实际 {corner}");
    }
}
