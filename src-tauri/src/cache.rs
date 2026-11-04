use crate::provider::Settings;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

const CAPACITY: usize = 48;

/// Remembers recent translations in memory only. Keys are hashes, nothing is
/// written to disk, and everything is gone when Claro quits.
#[derive(Default)]
pub struct TranslationCache {
    inner: Mutex<(HashMap<String, String>, VecDeque<String>)>,
}

fn key(s: &Settings, text: &str) -> String {
    let mut hash = Sha256::new();
    for part in [
        s.base_url.trim_end_matches('/'),
        &s.model,
        &s.target,
        &s.source_language,
        &s.tone,
        text,
    ] {
        hash.update(part.as_bytes());
        hash.update([0u8]);
    }
    hex::encode(hash.finalize())
}

impl TranslationCache {
    pub fn get(&self, s: &Settings, text: &str) -> Option<String> {
        self.inner.lock().ok()?.0.get(&key(s, text)).cloned()
    }
    pub fn put(&self, s: &Settings, text: &str, translation: &str) {
        let Ok(mut guard) = self.inner.lock() else { return };
        let k = key(s, text);
        if guard.0.insert(k.clone(), translation.to_string()).is_none() {
            guard.1.push_back(k);
        }
        while guard.1.len() > CAPACITY {
            if let Some(old) = guard.1.pop_front() {
                guard.0.remove(&old);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hit_miss_and_settings_isolation() {
        let cache = TranslationCache::default();
        let mut s = Settings::default();
        assert!(cache.get(&s, "hello").is_none());
        cache.put(&s, "hello", "hola");
        assert_eq!(cache.get(&s, "hello").as_deref(), Some("hola"));
        s.target = "French".into();
        assert!(cache.get(&s, "hello").is_none());
        s.target = "Spanish".into();
        s.tone = "Formal".into();
        assert!(cache.get(&s, "hello").is_none());
    }
    #[test]
    fn oldest_entries_are_evicted() {
        let cache = TranslationCache::default();
        let s = Settings::default();
        for i in 0..(CAPACITY + 5) {
            cache.put(&s, &format!("text {i}"), "x");
        }
        assert!(cache.get(&s, "text 0").is_none());
        assert!(cache.get(&s, &format!("text {}", CAPACITY + 4)).is_some());
    }
}
