use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::LPARAM;
use windows_sys::Win32::Graphics::Gdi::{
    CreateFontW, EnumFontFamiliesExW, GetDC, GetTextFaceW, ReleaseDC, SelectObject,
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, FF_DONTCARE,
    GB2312_CHARSET, LOGFONTW, OUT_DEFAULT_PRECIS, TEXTMETRICW,
};

pub const WEIGHT_NORMAL: i32 = 400;

pub const WEIGHT_BOLD: i32 = 700;

const FAMILIES: &[&str] = &[

    "Microsoft YaHei UI",

    "Microsoft YaHei",

    "HarmonyOS Sans SC",
    "MiSans",
    "SimHei",
];

static FACES: OnceLock<Vec<String>> = OnceLock::new();

static PICKED: OnceLock<Option<&'static str>> = OnceLock::new();

static CHOSEN: OnceLock<String> = OnceLock::new();

static LOGGED: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn collect(
    lf: *const LOGFONTW,
    _tm: *const TEXTMETRICW,
    _ty: u32,
    lp: LPARAM,
) -> i32 {
    let out = &mut *(lp as *mut Vec<String>);
    let p = (*lf).lfFaceName.as_ptr();
    let mut n = 0usize;
    while n < 32 && *p.add(n) != 0 {
        n += 1;
    }
    out.push(String::from_utf16_lossy(std::slice::from_raw_parts(p, n)));
    1
}

fn installed_faces() -> &'static [String] {
    FACES.get_or_init(|| {
        let mut names: Vec<String> = Vec::new();
        unsafe {

            let hdc = GetDC(std::ptr::null_mut());
            if hdc.is_null() {
                return names;
            }
            let mut lf: LOGFONTW = std::mem::zeroed();

            lf.lfCharSet = DEFAULT_CHARSET as u8;
            EnumFontFamiliesExW(hdc, &lf, Some(collect), &mut names as *mut _ as LPARAM, 0);
            ReleaseDC(std::ptr::null_mut(), hdc);
        }
        names
    })
}

fn pick<'a>(have: &[String], want: &[&'a str]) -> Option<&'a str> {
    want.iter()
        .copied()
        .find(|w| have.iter().any(|f| f.eq_ignore_ascii_case(w)))
}

fn picked_face() -> Option<&'static str> {
    *PICKED.get_or_init(|| pick(installed_faces(), FAMILIES))
}

fn chosen_face() -> &'static str {
    CHOSEN.get_or_init(|| {
        for key in ["HISHARE_FONT", "HISHARE_TOAST_FACE"] {
            if let Some(v) = crate::env::read(key) {
                let v = v.trim();
                if !v.is_empty() {
                    return v.to_string();
                }
            }
        }
        picked_face()
            .unwrap_or(crate::ui::theme::FACE)
            .to_string()
    })
}

unsafe fn log_actual(hf: *mut c_void) {
    if LOGGED.swap(true, Ordering::SeqCst) {
        return;
    }
    let want = chosen_face();
    let hdc = GetDC(std::ptr::null_mut());
    if hdc.is_null() {
        return;
    }
    let old = SelectObject(hdc, hf);
    let mut nb = [0u16; 64];
    let n = GetTextFaceW(hdc, 64, nb.as_mut_ptr());
    SelectObject(hdc, old);
    ReleaseDC(std::ptr::null_mut(), hdc);

    let end = (n.max(0) as usize).min(64);
    let end = end.min(nb.iter().position(|&c| c == 0).unwrap_or(end));
    let actual = String::from_utf16_lossy(&nb[..end]);
    if actual.eq_ignore_ascii_case(want) {
        crate::report::log(&format!("[提示] 字体：{actual}"));
    } else {
        crate::report::log(&format!(
            "[提示] 字体：{want} 取不到，系统换成了 {actual}"
        ));
    }
}

pub unsafe fn ui(px: i32, weight: i32) -> *mut c_void {
    let face = chosen_face();
    let name: Vec<u16> = face.encode_utf16().chain(std::iter::once(0)).collect();
    let hf = CreateFontW(
        -px.max(1),
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        GB2312_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        CLEARTYPE_QUALITY as u32,
        (DEFAULT_PITCH | FF_DONTCARE) as u32,
        name.as_ptr(),
    );
    log_actual(hf);
    hf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_match_win32() {

        assert_eq!(WEIGHT_NORMAL, 400);
        assert_eq!(WEIGHT_BOLD, 700);
    }

    fn have(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn prefers_the_font_everyone_has() {

        assert_eq!(
            pick(&have(&["HarmonyOS Sans SC", "Microsoft YaHei UI"]), FAMILIES),
            Some("Microsoft YaHei UI")
        );
        assert_eq!(
            pick(&have(&["MiSans", "Microsoft YaHei"]), FAMILIES),
            Some("Microsoft YaHei")
        );
    }

    #[test]
    fn falls_back_when_yahei_is_missing() {

        assert_eq!(
            pick(&have(&["HarmonyOS Sans SC", "SimSun"]), FAMILIES),
            Some("HarmonyOS Sans SC")
        );
        assert_eq!(
            pick(&have(&["SimHei", "SimSun"]), FAMILIES),
            Some("SimHei")
        );
    }

    #[test]
    fn picks_by_priority_not_by_install_order() {

        assert_eq!(
            pick(&have(&["SimHei", "Microsoft YaHei", "Microsoft YaHei UI"]), FAMILIES),
            Some("Microsoft YaHei UI")
        );
    }

    #[test]
    fn picks_nothing_when_none_are_installed() {

        assert_eq!(pick(&have(&["SimSun", "Arial"]), FAMILIES), None);
    }

    #[test]
    fn face_matching_ignores_case() {
        assert_eq!(
            pick(&have(&["microsoft yahei ui"]), FAMILIES),
            Some("Microsoft YaHei UI"),
            "族名大小写不同也得认出来"
        );
    }
}
