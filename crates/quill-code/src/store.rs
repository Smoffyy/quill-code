use std::fs;
use std::path::{Path, PathBuf};

use crate::config::app_dir;
use crate::thread::{Thread, now_ms};

fn threads_dir() -> PathBuf {
    app_dir().join("threads")
}

fn images_dir(id: u64) -> PathBuf {
    app_dir().join("images").join(id.to_string())
}

pub fn keep_image(id: u64, index: usize, source: &Path) -> Option<PathBuf> {
    let dir = images_dir(id);
    let ext = source.extension()?.to_string_lossy().to_lowercase();
    let target = dir.join(format!("{}-{index}.{ext}", now_ms()));
    fs::create_dir_all(&dir).and_then(|_| fs::copy(source, &target)).ok()?;
    Some(target)
}

pub fn load_threads() -> Vec<Thread> {
    let Ok(entries) = fs::read_dir(threads_dir()) else { return Vec::new() };
    let mut threads: Vec<Thread> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| fs::read_to_string(e.path()).ok())
        .filter_map(|text| serde_json::from_str::<Thread>(&text).ok())
        .map(|mut thread| {
            thread.recover();
            thread
        })
        .collect();
    threads.sort_by_key(|t| std::cmp::Reverse(t.updated));
    threads
}

pub fn save(thread: &Thread) {
    let path = threads_dir().join(format!("{}.json", thread.id));
    if let Ok(value) = serde_json::to_value(thread)
        && let Ok(text) = serde_json::to_string(&quill_config::keep_unknown(&path, value))
    {
        let _ = quill_config::write_atomic(&path, &text);
    }
}

pub fn delete(id: u64) {
    let _ = fs::remove_file(threads_dir().join(format!("{id}.json")));
    let _ = fs::remove_dir_all(images_dir(id));
}