use std::sync::OnceLock;

pub struct Progress {
    pub written: u64,
    pub total: u64,
    pub speed_mb: f64,
    pub peer_pct: Option<f64>,
}

type Logger = fn(&str);
type Painter = fn(&Progress);
type Ender = fn();

struct Sinks {
    log: Logger,
    paint: Painter,
    end: Ender,
}

static SINKS: OnceLock<Sinks> = OnceLock::new();

fn sinks() -> &'static Sinks {
    SINKS.get_or_init(|| Sinks {
        log: default_log,
        paint: default_paint,
        end: default_end,
    })
}

pub fn install(log: Logger, paint: Painter, end: Ender) -> bool {
    SINKS.set(Sinks { log, paint, end }).is_ok()
}

pub fn log(line: &str) {
    (sinks().log)(line);
}

pub fn progress(p: &Progress) {
    (sinks().paint)(p);
}

pub fn end_line() {
    (sinks().end)();
}

fn default_log(line: &str) {
    eprintln!("{line}");
}

fn default_paint(p: &Progress) {
    use std::io::Write;
    let pct = if p.total == 0 {
        0.0
    } else {
        p.written as f64 * 100.0 / p.total as f64
    };
    const WIDTH: usize = 24;
    let filled = (((pct / 100.0) * WIDTH as f64).round() as usize).min(WIDTH);
    let bar = format!("{}{}", "#".repeat(filled), "-".repeat(WIDTH - filled));
    let mb = p.written as f64 / 1_048_576.0;
    let total_mb = p.total as f64 / 1_048_576.0;
    match p.peer_pct {
        None => print!("\r[{bar}] {pct:5.1}%  {mb:>7.1}/{total_mb:.1} MB  {:>5.1} MB/s", p.speed_mb),
        Some(peer) => print!(
            "\r[{bar}] {pct:5.1}%  {mb:>7.1}/{total_mb:.1} MB  {:>5.1} MB/s  对方 {peer:5.1}%",
            p.speed_mb
        ),
    }
    let _ = std::io::stdout().flush();
}

fn default_end() {
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_sinks_are_usable_without_install() {

        progress(&Progress {
            written: 1,
            total: 2,
            speed_mb: 0.5,
            peer_pct: None,
        });
        end_line();
        log("");
    }
}
