/// Stores only the current selection in memory. No history or disk persistence.
#[derive(Default)]
pub struct SelectionTracker {
    generation: u64,
    current: String,
    dismissed: String,
}
impl SelectionTracker {
    pub fn select(&mut self, app: &str, text: &str) -> Option<u64> {
        let identity = format!("{}\0{}", app, text);
        if self.current == identity || self.dismissed == identity {
            return None;
        }
        self.current = identity;
        self.generation += 1;
        Some(self.generation)
    }
    pub fn dismiss(&mut self, remember: bool) {
        self.generation += 1;
        if remember {
            self.dismissed = self.current.clone();
        }
    }
    pub fn clear(&mut self) {
        self.generation += 1;
        self.current.clear();
        self.dismissed.clear();
    }
    pub fn is_current(&self, generation: u64) -> bool {
        self.generation == generation
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn latest_selection_invalidates_older_results() {
        let mut s = SelectionTracker::default();
        let first = s.select("editor", "first").unwrap();
        let next = s.select("editor", "second").unwrap();
        assert!(!s.is_current(first));
        assert!(s.is_current(next));
    }
    #[test]
    fn dismiss_does_not_immediately_reshow_same_selection() {
        let mut s = SelectionTracker::default();
        let id = s.select("editor", "hello").unwrap();
        s.dismiss(true);
        assert!(!s.is_current(id));
        assert!(s.select("editor", "hello").is_none());
        assert!(s.select("editor", "different").is_some());
    }
    #[test]
    fn cleared_selection_can_be_selected_again() {
        let mut s = SelectionTracker::default();
        s.select("editor", "hello");
        s.dismiss(true);
        s.clear();
        assert!(s.select("editor", "hello").is_some());
    }
    #[test]
    fn permission_loss_lock_and_pause_invalidate_inflight_result() {
        for _ in ["permission", "lock", "pause"] {
            let mut s = SelectionTracker::default();
            let id = s.select("editor", "private").unwrap();
            s.clear();
            assert!(!s.is_current(id));
            assert!(s.current.is_empty());
            assert!(s.dismissed.is_empty());
        }
    }
    #[test]
    fn bounds_updates_do_not_retranslate_same_text() {
        let mut s = SelectionTracker::default();
        let generation = s.select("editor", "hello").unwrap();
        assert!(s.select("editor", "hello").is_none());
        assert!(s.is_current(generation));
        assert!(s.select("browser", "hello").is_some());
    }
}
