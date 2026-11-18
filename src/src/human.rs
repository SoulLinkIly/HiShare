pub fn bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let v = n as f64;
    if v >= GB {
        format!("{:.2} GB", v / GB)
    } else if v >= MB {
        format!("{:.1} MB", v / MB)
    } else if v >= KB {
        format!("{:.1} KB", v / KB)
    } else {
        format!("{n} B")
    }
}

#[cfg(test)]
mod tests {
    use super::bytes;

    #[test]
    fn picks_a_unit_that_never_rounds_to_zero() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(512), "512 B");

        assert_eq!(bytes(512 * 1024), "512.0 KB");
        assert_eq!(bytes(1024 * 1024), "1.0 MB");
        assert_eq!(bytes(13 * 1024 * 1024), "13.0 MB");
        assert_eq!(bytes(3 * 1024 * 1024 * 1024), "3.00 GB");
    }

    #[test]
    fn boundary_steps_up_a_unit() {
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1024), "1.0 KB");
        assert_eq!(bytes(1024 * 1024 - 1), "1024.0 KB", "差 1 字节仍按 KB 记");
    }
}
