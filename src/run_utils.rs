use rand::RngExt;
use std::collections::HashSet;
use std::fs;
use std::time::Duration;

pub fn init_concurrency(threads: usize, total: usize) -> usize {
    threads.min(total).max(1)
}

pub async fn random_sleep_s(log_name: &str, min_s: u64, max_s: u64) {
    let s = rand::rng().random_range(min_s..=max_s);
    log::info!("{} will sleep {s}s before start", log_name);
    tokio::time::sleep(Duration::from_secs(s)).await;
}

pub fn remove_lines_trimmed(path: &str, remove: &HashSet<String>) -> Result<(), String> {
    let src = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut out = String::new();
    for line in src.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if remove.contains(t) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    fs::write(path, out).map_err(|e| e.to_string())?;
    Ok(())
}
