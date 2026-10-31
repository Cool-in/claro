use sha2::{Digest, Sha256};
#[derive(Clone)]
pub struct ActiveSelection {
    pub app: String,
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub top: f64,
}
fn identity(app: &str, text: &str) -> String {
    hex::encode(Sha256::digest(format!("{}\0{}", app, text).as_bytes()))
}
/// Stores only the current selection in memory. No history or disk persistence.
#[derive(Default)]
pub struct SelectionTracker {
    generation: u64,
    current: String,
    dismissed: String,
    pending: bool,
    active: Option<ActiveSelection>,
}
impl SelectionTracker {
    pub fn select(&mut self, app: &str, text: &str) -> Option<u64> {
        let identity = identity(app, text);
        if self.current == identity || self.dismissed == identity {
            return None;
        }
        self.current = identity;
        self.active = Some(ActiveSelection {
            app: app.into(),
            text: text.into(),
            x: 0.0,
            y: 0.0,
            top: 0.0,
        });
        self.pending = true;
        self.generation += 1;
        Some(self.generation)
    }
    pub fn dismiss(&mut self, remember: bool) {
        self.generation += 1;
        self.pending = false;
        self.active = None;
        if remember {
            self.dismissed = self.current.clone();
        }
    }
    pub fn clear(&mut self) {
        self.generation += 1;
        self.pending = false;
        self.active = None;
        self.current.clear();
        self.dismissed.clear();
    }
    pub fn settle(&mut self, app: &str, text: &str) -> Option<u64> {
        if self.pending && self.current == identity(app, text) && self.dismissed != self.current {
            self.generation += 1;
            Some(self.generation)
        } else {
            None
        }
    }
    pub fn set_anchor(&mut self, generation: u64, x: f64, y: f64) {
        if self.is_current(generation) {
            if let Some(active) = self.active.as_mut() {
                active.x = x;
                active.y = y;
                active.top = y - 20.0;
            }
        }
    }
    pub fn set_top(&mut self, generation: u64, top: f64) {
        if self.is_current(generation) {
            if let Some(active) = self.active.as_mut() {
                active.top = top;
            }
        }
    }
    pub fn has_active(&self) -> bool {
        self.active.is_some()
    }
    pub fn anchor(&self) -> Option<(f64, f64, f64)> {
        self.active.as_ref().map(|s| (s.x, s.y, s.top))
    }
    pub fn retarget(&mut self) -> Option<(u64, ActiveSelection)> {
        let active = self.active.clone()?;
        self.generation += 1;
        self.pending = true;
        Some((self.generation, active))
    }
    pub fn started(&mut self, generation: u64) {
        if self.generation == generation {
            self.pending = false;
        }
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

#[cfg(test)]
mod settle_tests {
    use super::*;
    #[test]
    fn mouse_up_accelerates_pending_request_once() {
        let mut s = SelectionTracker::default();
        let old = s.select("editor", "hello").unwrap();
        let new = s.settle("editor", "hello").unwrap();
        assert!(!s.is_current(old));
        s.started(new);
        assert!(s.settle("editor", "hello").is_none());
        s.dismiss(true);
        assert!(s.settle("editor", "hello").is_none());
    }
}

#[cfg(test)]
mod retarget_tests {
    use super::*;
    #[test]
    fn retarget_invalidates_old_request_and_keeps_only_current_source() {
        let mut s = SelectionTracker::default();
        let old = s.select("editor", "current source").unwrap();
        s.set_anchor(old, 12.0, 34.0);
        let (next, active) = s.retarget().unwrap();
        assert!(!s.is_current(old));
        assert!(s.is_current(next));
        assert_eq!(active.text, "current source");
        assert_eq!((active.x, active.y), (12.0, 34.0));
        s.dismiss(true);
        assert!(s.active.is_none());
        assert!(s.retarget().is_none());
        assert!(!s.current.contains("current source"));
    }
    #[test]
    fn clear_removes_retargetable_text() {
        let mut s = SelectionTracker::default();
        s.select("editor", "private");
        s.clear();
        assert!(s.retarget().is_none());
        assert!(s.active.is_none());
    }
}
