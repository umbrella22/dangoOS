//! Associate the single Settings service request with its originating page.
use crate::shell::navigation::router::Session;

pub(crate) struct Completion {
    pub session: Option<Session>,
    pub failed: bool,
}
pub(crate) struct Operations {
    revision: u32,
    pending: Option<Session>,
    skin: Option<Session>,
}
impl Operations {
    pub fn new(revision: u32) -> Self {
        Self {
            revision,
            pending: None,
            skin: None,
        }
    }
    /// The accepted operation remains occupied until its completion is read,
    /// even if the service becomes idle between reading and submitting.
    pub fn submit(&mut self, session: Session, skin: bool, enqueue: impl FnOnce() -> bool) -> bool {
        if self.pending.is_some() || !enqueue() {
            return false;
        }
        self.pending = Some(session);
        if skin {
            self.skin = Some(session);
        }
        true
    }
    pub fn complete(&mut self, revision: u32) -> Option<Completion> {
        if revision == self.revision {
            return None;
        }
        self.revision = revision;
        let failed = revision & 1 != 0;
        if failed {
            self.skin = None;
        }
        Some(Completion {
            session: self.pending.take(),
            failed,
        })
    }
    /// Resource activation and service completion can be observed on different
    /// UI iterations. Both refer to the original selection page.
    pub fn skin_activated(&mut self) -> Option<Session> {
        self.skin.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::navigation::{route::Route, router::Router};

    #[test]
    fn an_unread_completion_cannot_be_assigned_to_a_new_request() {
        let mut router = Router::new();
        let old = router.session();
        let mut operations = Operations::new(0);
        assert!(operations.submit(old, false, || true));
        // RESULT was read as zero; the worker now completes and becomes idle.
        assert!(operations.complete(0).is_none());
        router.navigate(Route::Camera);
        let new = router.session();
        assert!(!operations.submit(new, false, || panic!("old receipt has not been collected")));
        let result = operations.complete(2).unwrap();
        assert_eq!(result.session, Some(old));
        assert!(!router.accepts(result.session.unwrap()));
        assert!(!result.failed);
        assert!(operations.submit(new, false, || true));
        let result = operations.complete(5).unwrap();
        assert_eq!(result.session, Some(new));
        assert!(result.failed);
        assert!(operations.complete(5).is_none());
    }
    #[test]
    fn rejected_requests_do_not_replace_a_selection_or_its_activation_session() {
        let mut router = Router::new();
        let old = router.session();
        let mut operations = Operations::new(0);
        assert!(!operations.submit(old, true, || false));
        assert!(operations.skin_activated().is_none());
        assert!(operations.submit(old, true, || true));
        router.navigate(Route::Camera);
        assert!(!operations.submit(router.session(), true, || true));
        assert_eq!(operations.complete(2).unwrap().session, Some(old));
        assert_eq!(operations.skin_activated(), Some(old));
        assert!(operations.skin_activated().is_none());
        assert!(operations.submit(router.session(), true, || true));
        assert!(operations.complete(5).unwrap().failed);
        assert!(operations.skin_activated().is_none());
    }
}
