use std::sync::Arc;

use super::updater_thread::{Session, Updater};

/// A reader over the Updater's current session, rebuilt when the session changes. One thread only.
pub struct SessionReader<R> {
    source: Box<dyn Fn() -> Option<Arc<Session>> + Send>,
    make: Box<dyn Fn(&Arc<Session>) -> R + Send>,
    sess: Option<Arc<Session>>,
    reader: Option<R>,
}

impl<R: Send> SessionReader<R> {
    pub fn new(
        source: impl Fn() -> Option<Arc<Session>> + Send + 'static,
        make: impl Fn(&Arc<Session>) -> R + Send + 'static,
    ) -> Self {
        Self {
            source: Box::new(source),
            make: Box::new(make),
            sess: None,
            reader: None,
        }
    }

    pub fn for_updater(u: &Arc<Updater>, make: impl Fn(&Arc<Session>) -> R + Send + 'static) -> Self {
        let u = u.clone();
        Self::new(move || u.session(), make)
    }

    /// None without a session. The old reader is dropped before the old session is released.
    pub fn get(&mut self) -> Option<&mut R> {
        let s = (self.source)();
        let same = match (&s, &self.sess) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (a, b) => a.is_none() && b.is_none(),
        };
        if !same {
            self.reader = None; // it references the old session, so it goes first
            self.sess = s;
            self.reader = self.sess.as_ref().map(|s| (self.make)(s));
        }
        self.reader.as_mut()
    }

    pub fn session(&self) -> Option<&Arc<Session>> {
        self.sess.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Mutex, Weak};

    #[derive(Default)]
    struct Counts {
        built: i32,
        alive: i32,
        watched: Weak<Session>, // the session the reader was built from
        session_alive_at_destroy: bool,
    }

    struct CountingReader(Arc<Mutex<Counts>>);
    impl CountingReader {
        fn new(c: &Arc<Mutex<Counts>>) -> Self {
            let mut n = c.lock().unwrap();
            n.built += 1;
            n.alive += 1;
            Self(c.clone())
        }
    }
    impl Drop for CountingReader {
        fn drop(&mut self) {
            let mut n = self.0.lock().unwrap();
            n.alive -= 1;
            n.session_alive_at_destroy = n.session_alive_at_destroy && n.watched.upgrade().is_some();
        }
    }

    #[test]
    fn app_session_reader_rebuilds_on_session_change() {
        let (s1, s2) = (Arc::new(Session::default()), Arc::new(Session::default()));
        fn assert_send<T: Send>(_: &T) {}
        // from here the reader is the only owner of s1 besides the queue's copies
        let q = Mutex::new(VecDeque::from([Some(s1.clone()), Some(s1), Some(s2), None]));
        let counts = Arc::new(Mutex::new(Counts {
            session_alive_at_destroy: true,
            ..Counts::default()
        }));
        let c = counts.clone();
        let mut r = SessionReader::new(
            move || q.lock().unwrap().pop_front().unwrap(),
            move |_| CountingReader::new(&c),
        );
        assert_send(&r); // vbl moves a reader into its engine thread
        // Track s1 via the first get(): afterwards r.session() is its only owner.
        let first: *const CountingReader = r.get().unwrap();
        counts.lock().unwrap().watched = Arc::downgrade(r.session().unwrap());
        assert_eq!(counts.lock().unwrap().built, 1);
        assert_eq!(r.get().map(|x| x as *const _), Some(first));
        assert_eq!(counts.lock().unwrap().built, 1);
        assert!(r.get().is_some()); // s2
        assert_eq!(counts.lock().unwrap().built, 2);
        assert_eq!(counts.lock().unwrap().alive, 1);
        assert!(counts.lock().unwrap().session_alive_at_destroy);
        assert!(r.get().is_none());
        assert_eq!(counts.lock().unwrap().alive, 0);
    }
}
