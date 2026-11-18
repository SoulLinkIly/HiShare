use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn local_offset_secs() -> i64 {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::SYSTEMTIME;
        use windows_sys::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};

        unsafe fn to_secs(t: SYSTEMTIME) -> i64 {
            let days = days_from_civil(t.wYear as i64, t.wMonth as i64, t.wDay as i64);
            days * 86_400
                + t.wHour as i64 * 3600
                + t.wMinute as i64 * 60
                + t.wSecond as i64
        }

        let mut local: SYSTEMTIME = unsafe { std::mem::zeroed() };
        let mut utc: SYSTEMTIME = unsafe { std::mem::zeroed() };
        unsafe {
            GetLocalTime(&mut local);
            GetSystemTime(&mut utc);
            to_secs(local) - to_secs(utc)
        }
    }
    #[cfg(not(windows))]
    {
        0
    }
}

pub fn format_at(secs: i64, offset: i64) -> String {
    format_parts(secs, offset, '-')
}

pub fn format_at_wire(secs: i64, offset: i64) -> String {
    format_parts(secs, offset, ':')
}

fn format_parts(secs: i64, offset: i64, sep: char) -> String {
    let local = secs.saturating_add(offset);
    let days = local.div_euclid(86_400);
    let sod = local.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}{sep}{m:02}{sep}{d:02} {:02}:{:02}:{:02}",
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60
    )
}

pub fn format_unix(secs: i64) -> String {
    format_at(secs, local_offset_secs())
}

pub fn format_unix_ms(ms: u64) -> String {
    format_unix((ms / 1000) as i64)
}

pub fn format_unix_ms_wire(ms: u64) -> String {
    format_at_wire((ms / 1000) as i64, local_offset_secs())
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(windows)]
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_epoch_formats_as_1970() {
        assert_eq!(format_at(0, 0), "1970-01-01 00:00:00");
    }

    #[test]
    fn known_instant_with_offset() {

        let secs = 1_793_961_012;
        assert_eq!(format_at(secs, 0), "2026-11-06 10:30:12");
        assert_eq!(format_at(secs, 8 * 3600), "2026-11-06 18:30:12");
    }

    #[test]
    fn leap_day_and_year_rollover() {
        assert_eq!(format_at(1_709_164_800, 0), "2024-02-29 00:00:00");
        assert_eq!(format_at(1_735_689_599, 0), "2024-12-31 23:59:59");
        assert_eq!(format_at(1_735_689_600, 0), "2025-01-01 00:00:00");
    }

    #[test]
    fn negative_offset_can_go_back_a_day() {

        let secs = 1_767_236_400;
        assert_eq!(format_at(secs, -5 * 3600), "2025-12-31 22:00:00");
    }

    #[test]
    fn wire_format_keeps_colons() {

        assert_eq!(format_at_wire(1_793_961_012, 8 * 3600), "2026:11:06 18:30:12");
    }

    #[test]
    fn millis_truncate_to_seconds() {
        let off = local_offset_secs();
        assert_eq!(format_unix_ms(0), format_at(0, off));
        assert_eq!(format_unix_ms(999), format_at(0, off), "不足一秒不进位");
        assert_eq!(format_unix_ms(1000), format_at(1, off));
    }
}
