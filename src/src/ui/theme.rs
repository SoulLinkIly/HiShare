pub const BG: u32 = 0x00FF_FFFF;

pub const MAIN: u32 = 0x001A_1A1A;

pub const SUB: u32 = 0x0093_8E8E;

pub const LINE: u32 = 0x00EA_E5E5;

pub const HOVER: u32 = 0x00F7_F2F2;

pub const ACCENT: u32 = 0x00C0_67_00;

pub const ERROR: u32 = 0x0030_3BFF;

pub const MEDIA: u32 = 0x0035_C734;
pub const FILE_BODY: u32 = 0x00F6_F5F7;
pub const FILE_LINE: u32 = 0x00D6_D4DA;
pub const ASK: u32 = 0x0078_747D;

pub const ICON_BG: u32 = 0x00FF_F2EA;

pub const BORDER: u32 = 0x00E6_E2E9;

pub const HINT: u32 = 0x00B4_B0B8;

pub const BTN_BG: u32 = 0x00E1_DDF9;
pub const BTN_TEXT: u32 = 0x007A_70C2;
pub const BTN_HOVER_BG: u32 = 0x00EA_E7FC;

pub const RADIUS: i32 = 22;

pub const FACE: &str = "Microsoft YaHei UI";

pub fn px(v: i32, dpi: i32) -> i32 {
    v * dpi.max(96) / 96
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn px_scales_with_dpi() {
        assert_eq!(px(20, 96), 20);
        assert_eq!(px(20, 120), 25);
        assert_eq!(px(20, 0), 20, "dpi 取不到时按 96 处理，不能除出 0");
    }

    #[test]
    fn palette_is_consistent() {

        assert_eq!(SUB, 0x0093_8E8E);
        assert_eq!(RADIUS, 22);
    }
}
