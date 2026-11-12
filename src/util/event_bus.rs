use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};

struct Slot {
    ty: TypeId,
    f: Box<dyn Fn(&dyn Any) + Send + Sync>,
    state: Mutex<SlotState>,
    cv: Condvar,
}

struct SlotState {
    running: i32, // calls in progress, across threads
    live: bool,
}

#[derive(Default)]
struct Inner {
    handlers: Mutex<HashMap<TypeId, Vec<Arc<Slot>>>>,
}

struct Pending {
    bus: Arc<Inner>,
    ty: TypeId,
    event: Box<dyn Any>,
}

thread_local! {
    // Per thread, not per bus: an emit from a handler waits until the outermost emit's handlers are done.
    static DISPATCHING: Cell<bool> = const { Cell::new(false) };
    static PENDING: RefCell<VecDeque<Pending>> = const { RefCell::new(VecDeque::new()) };
    // Nested emits are queued, so a thread runs at most one handler at a time.
    static RUNNING: Cell<*const Slot> = const { Cell::new(std::ptr::null()) };
}

/// Handle to one handler. `reset()` (or drop) unsubscribes; once it returns the handler never runs again: it waits
/// for a call of that handler running on another thread, but not for one on the calling thread (so a handler may
/// reset its own Subscription). Two handlers that reset each other while running on two threads deadlock.
#[derive(Default)]
pub struct Subscription(Option<(Arc<Inner>, Arc<Slot>)>);

impl Subscription {
    pub fn reset(&mut self) {
        let Some((bus, slot)) = self.0.take() else { return };
        if let Some(v) = bus.handlers.lock().unwrap().get_mut(&slot.ty) {
            v.retain(|s| !Arc::ptr_eq(s, &slot));
        }
        // An emitter may already hold the slot in its snapshot; `live` stops it, and we wait out calls already started.
        let mut st = slot.state.lock().unwrap();
        st.live = false;
        let own = i32::from(RUNNING.get() == Arc::as_ptr(&slot)); // don't wait for our own call
        while st.running != own {
            st = slot.cv.wait(st).unwrap();
        }
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.reset();
    }
}

/// Typed publish/subscribe, keyed by event type. Thread-safe; clones share one bus. Handlers run on the emitting
/// thread, in subscription order, outside the bus lock. An emit from inside a handler (any bus, same thread) is
/// queued and delivered, FIFO, after the current event's handlers finish.
#[derive(Clone, Default)]
pub struct EventBus(Arc<Inner>);

impl EventBus {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use = "dropping the Subscription unsubscribes"]
    pub fn on<E: 'static>(&self, f: impl Fn(&E) + Send + Sync + 'static) -> Subscription {
        let ty = TypeId::of::<E>();
        let slot = Arc::new(Slot {
            ty,
            f: Box::new(move |e| f(e.downcast_ref::<E>().expect("event type matches its TypeId"))),
            state: Mutex::new(SlotState { running: 0, live: true }),
            cv: Condvar::new(),
        });
        self.0
            .handlers
            .lock()
            .unwrap()
            .entry(ty)
            .or_default()
            .push(slot.clone());
        Subscription(Some((self.0.clone(), slot)))
    }

    pub fn emit<E: 'static>(&self, e: E) {
        let ty = TypeId::of::<E>();
        if DISPATCHING.get() {
            PENDING.with_borrow_mut(|p| {
                p.push_back(Pending {
                    bus: self.0.clone(),
                    ty,
                    event: Box::new(e),
                })
            });
            return;
        }
        DISPATCHING.set(true);
        struct Guard;
        impl Drop for Guard {
            fn drop(&mut self) {
                DISPATCHING.set(false);
                PENDING.with_borrow_mut(VecDeque::clear); // only non-empty if a handler panicked
            }
        }
        let _guard = Guard;
        dispatch(&self.0, ty, &e);
        while let Some(p) = PENDING.with_borrow_mut(VecDeque::pop_front) {
            dispatch(&p.bus, p.ty, p.event.as_ref());
        }
    }

    /// Collects every E emitted from now on; stays subscribed while the Inbox lives.
    /// Beyond `cap` (at least 1) the oldest entry is dropped.
    #[must_use]
    pub fn inbox<E: Clone + Send + 'static>(&self, cap: usize) -> Inbox<E> {
        let q = Arc::new(Mutex::new(VecDeque::new()));
        let (sink, cap) = (q.clone(), cap.max(1));
        let sub = self.on(move |e: &E| {
            let mut q = sink.lock().unwrap();
            if q.len() >= cap {
                q.pop_front();
            }
            q.push_back(e.clone());
        });
        Inbox { q, _sub: sub }
    }
}

fn dispatch(bus: &Inner, ty: TypeId, e: &dyn Any) {
    // Copied so handlers run without the bus lock.
    let Some(snapshot) = bus.handlers.lock().unwrap().get(&ty).cloned() else {
        return;
    };
    for slot in snapshot {
        {
            let mut st = slot.state.lock().unwrap();
            if !st.live {
                continue;
            }
            st.running += 1;
        }
        struct Done<'a>(&'a Slot);
        impl Drop for Done<'_> {
            fn drop(&mut self) {
                RUNNING.set(std::ptr::null());
                self.0.state.lock().unwrap().running -= 1;
                self.0.cv.notify_all();
            }
        }
        let _done = Done(&slot);
        RUNNING.set(Arc::as_ptr(&slot));
        (slot.f)(e);
    }
}

/// Bounded event queue filled by the bus, drained by its owner (e.g. a UI or engine thread).
pub struct Inbox<E> {
    q: Arc<Mutex<VecDeque<E>>>,
    _sub: Subscription,
}

impl<E> Inbox<E> {
    pub fn drain(&self) -> Vec<E> {
        self.q.lock().unwrap().drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering::Relaxed};
    use std::time::{Duration, Instant};

    #[derive(Clone)]
    struct Ping(i32);
    struct Pong;

    fn log() -> Arc<Mutex<Vec<String>>> {
        Arc::default()
    }

    #[test]
    fn bus_handlers_run_in_order() {
        let (bus, got) = (EventBus::new(), log());
        let (g1, g2) = (got.clone(), got.clone());
        let _a = bus.on(move |_: &Ping| g1.lock().unwrap().push("1".into()));
        let _b = bus.on(move |_: &Ping| g2.lock().unwrap().push("2".into()));
        bus.emit(Ping(0));
        bus.emit(Pong);
        assert_eq!(*got.lock().unwrap(), ["1", "2"]);
    }

    #[test]
    fn bus_subscription_unsubscribes() {
        let bus = EventBus::new();
        let calls = Arc::new(AtomicI32::new(0));
        let count = |c: &Arc<AtomicI32>| {
            let c = c.clone();
            move |_: &Ping| {
                c.fetch_add(1, Relaxed);
            }
        };
        let mut sub = bus.on(count(&calls));
        bus.emit(Ping(0));
        assert_eq!(calls.load(Relaxed), 1);
        sub.reset();
        bus.emit(Ping(0));
        assert_eq!(calls.load(Relaxed), 1);
        {
            let scoped = bus.on(count(&calls));
            let _moved = scoped;
            bus.emit(Ping(0));
            assert_eq!(calls.load(Relaxed), 2);
        }
        bus.emit(Ping(0));
        assert_eq!(calls.load(Relaxed), 2);
        // Resetting a subscription from inside its own handler must not deadlock.
        let own: Arc<Mutex<Subscription>> = Arc::default();
        let (c, o) = (calls.clone(), own.clone());
        let sub = bus.on(move |_: &Ping| {
            c.fetch_add(1, Relaxed);
            let taken = std::mem::take(&mut *o.lock().unwrap());
            drop(taken);
        });
        *own.lock().unwrap() = sub;
        bus.emit(Ping(0));
        bus.emit(Ping(0));
        assert_eq!(calls.load(Relaxed), 3);
    }

    #[test]
    fn bus_emit_inside_handler_is_delivered_after() {
        let (bus, got) = (EventBus::new(), log());
        let (g, b) = (got.clone(), bus.clone());
        let _a = bus.on(move |p: &Ping| {
            g.lock().unwrap().push(format!("ping{}", p.0));
            if p.0 == 1 {
                b.emit(Pong);
            }
            g.lock().unwrap().push("end".into());
        });
        let g = got.clone();
        let _b = bus.on(move |_: &Pong| g.lock().unwrap().push("pong".into()));
        bus.emit(Ping(1));
        assert_eq!(*got.lock().unwrap(), ["ping1", "end", "pong"]);
    }

    #[test]
    fn bus_inbox_collects_across_threads() {
        let bus = EventBus::new();
        let inbox = bus.inbox::<Ping>(1024);
        let b = bus.clone();
        std::thread::spawn(move || (0..100).for_each(|i| b.emit(Ping(i))))
            .join()
            .unwrap();
        let got = inbox.drain();
        assert_eq!(got.len(), 100);
        assert!(got.iter().enumerate().all(|(i, p)| p.0 == i as i32));
        assert!(inbox.drain().is_empty());
        drop(inbox);
        bus.emit(Ping(0)); // the inbox unsubscribed when dropped
    }

    #[test]
    fn bus_inbox_drops_oldest_beyond_cap() {
        let bus = EventBus::new();
        let inbox = bus.inbox::<Ping>(3);
        (1..=5).for_each(|i| bus.emit(Ping(i)));
        assert_eq!(inbox.drain().iter().map(|p| p.0).collect::<Vec<_>>(), [3, 4, 5]);
    }

    #[test]
    fn bus_unsubscribe_during_emit_from_other_thread() {
        let bus = EventBus::new();
        let stop = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicBool::new(false));
        let called_after_reset = Arc::new(AtomicBool::new(false));
        let t0 = Instant::now();
        let (b, s, st) = (bus.clone(), stop.clone(), started.clone());
        let emitter = std::thread::spawn(move || {
            while !s.load(Relaxed) {
                b.emit(Ping(0));
                st.store(true, Relaxed);
            }
        });
        while !started.load(Relaxed) {
            std::thread::yield_now();
        }
        for _ in 0..1000 {
            let dead = Arc::new(AtomicBool::new(false));
            let called = Arc::new(AtomicBool::new(false));
            let (d, c, bad) = (dead.clone(), called.clone(), called_after_reset.clone());
            let mut sub = bus.on(move |_: &Ping| {
                c.store(true, Relaxed);
                std::thread::yield_now(); // widen the window between "handler started" and the check
                if d.load(Relaxed) {
                    bad.store(true, Relaxed);
                }
            });
            // Reset only once the emitter is dispatching to this handler, so reset races a live call.
            for _ in 0..100_000 {
                if called.load(Relaxed) {
                    break;
                }
                std::thread::yield_now();
            }
            sub.reset();
            dead.store(true, Relaxed);
        }
        while t0.elapsed() < Duration::from_millis(200) {
            std::thread::yield_now();
        }
        stop.store(true, Relaxed);
        emitter.join().unwrap();
        assert!(!called_after_reset.load(Relaxed));
    }
}
