//! Roblox gives no events (we only read its memory), so `RobloxWatcher` polls and emits these on an `EventBus`.
use std::sync::atomic::{AtomicBool, AtomicI8, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::instance::{child_by_class, data_model};
use super::memory::Memory;
use super::offsets::{Offsets, SDK_KEYS};
use super::read::{camera_subject, character_of, current_camera, local_player, ptr_at};
use crate::app::updater_thread::{AppStatus, Session, Updater};
use crate::util::event_bus::EventBus;

#[derive(Clone, Debug)]
pub struct RobloxOpened;
#[derive(Clone, Debug)]
pub struct RobloxClosed;
#[derive(Clone, Debug)]
pub struct StatusChanged {
    pub from: AppStatus,
    pub to: AppStatus,
    pub detail: String,
}
/// Also re-emitted (without SessionLost) when the session is replaced by revalidated offsets.
#[derive(Clone)]
pub struct SessionReady {
    pub session: Arc<Session>,
}
#[derive(Clone, Debug)]
pub struct SessionLost;
/// Emitted by ForegroundWatcher on the UI thread, not the watcher's polling thread.
#[derive(Clone, Debug)]
pub struct RobloxFocusChanged {
    pub focused: bool,
}
#[derive(Clone, Debug)]
pub struct ChatFocusChanged {
    /// None = unreadable.
    pub focused: Option<bool>,
}
#[derive(Clone, Debug)]
pub struct CharacterChanged {
    pub before: usize,
    pub after: usize,
}
#[derive(Clone, Debug)]
pub struct CameraSubjectChanged {
    pub before: usize,
    pub after: usize,
}

/// Offsets each state group reads; a group with any key missing is skipped.
pub const CHAT_KEYS: [&str; 2] = ["UserInputService.WindowInputState", "WindowInputState.CurrentTextBox"];
pub const CHARACTER_KEYS: [&str; 2] = ["Player.LocalPlayer", "Player.ModelInstance"];
pub(crate) const CAMERA_KEYS: [&str; 3] = ["DataModel.Workspace", "Workspace.CurrentCamera", "Camera.CameraSubject"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RobloxRoots {
    pub data_model: usize,
    pub players: usize,
    pub user_input_service: usize,
}

pub(crate) fn resolve_roots(m: &dyn Memory, o: &Offsets, dm: usize) -> RobloxRoots {
    let mut r = RobloxRoots {
        data_model: dm,
        ..RobloxRoots::default()
    };
    if dm != 0 {
        r.players = child_by_class(m, o, dm, "Players");
        r.user_input_service = child_by_class(m, o, dm, "UserInputService");
    }
    r
}

/// Roots plus the earliest time a missing-child retry may walk the tree again (None: any time).
#[derive(Clone, Debug, Default)]
pub struct RootsCache {
    pub roots: RobloxRoots,
    pub retry_at: Option<Instant>,
}

/// Re-resolves at once when dm changed; when players/user_input_service is still 0, at most once per second.
/// Returns whether it re-resolved.
pub fn update_roots(c: &mut RootsCache, m: &dyn Memory, o: &Offsets, dm: usize, now: Instant) -> bool {
    let missing = c.roots.players == 0 || c.roots.user_input_service == 0;
    if dm == c.roots.data_model && !(missing && c.retry_at.is_none_or(|t| now >= t)) {
        return false;
    }
    c.roots = resolve_roots(m, o, dm);
    c.retry_at = Some(now + Duration::from_secs(1));
    true
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RobloxState {
    /// CurrentTextBox != 0; None when unreadable.
    pub chat_focused: Option<bool>,
    pub character: usize,
    pub camera_subject: usize,
}

/// Which groups read_roblox_state reads (a disabled group stays default).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RobloxGroups {
    pub chat: bool,
    pub character: bool,
    pub camera: bool,
}

impl Default for RobloxGroups {
    fn default() -> Self {
        Self {
            chat: true,
            character: true,
            camera: true,
        }
    }
}

/// A group is on only when all its keys are in `wanted`.
pub(crate) fn groups_for(wanted: &[&str]) -> RobloxGroups {
    let all = |group: &[&str]| group.iter().all(|k| wanted.contains(k));
    RobloxGroups {
        chat: all(&CHAT_KEYS),
        character: all(&CHARACTER_KEYS),
        camera: all(&CAMERA_KEYS),
    }
}

pub fn read_roblox_state(m: &dyn Memory, o: &Offsets, r: &RobloxRoots, g: RobloxGroups) -> RobloxState {
    let mut s = RobloxState::default();
    if g.chat && r.user_input_service != 0 && o.has_all(&CHAT_KEYS) {
        let wis = ptr_at(m, o.at(r.user_input_service, "UserInputService.WindowInputState"));
        if wis != 0 {
            s.chat_focused = m
                .get::<usize>(o.at(wis, "WindowInputState.CurrentTextBox"))
                .map(|b| b != 0);
        }
    }
    if g.character && r.players != 0 && o.has_all(&CHARACTER_KEYS) {
        s.character = character_of(m, o, local_player(m, o, r.players));
    }
    if g.camera && r.data_model != 0 && o.has_all(&CAMERA_KEYS) {
        s.camera_subject = camera_subject(m, o, current_camera(m, o, r.data_model));
    }
    s
}

/// Emits ChatFocusChanged, CharacterChanged, CameraSubjectChanged (in that order) for each field that differs.
pub(crate) fn emit_changes(b: &RobloxState, a: &RobloxState, bus: &EventBus) {
    if b.chat_focused != a.chat_focused {
        bus.emit(ChatFocusChanged {
            focused: a.chat_focused,
        });
    }
    if b.character != a.character {
        bus.emit(CharacterChanged {
            before: b.character,
            after: a.character,
        });
    }
    if b.camera_subject != a.camera_subject {
        bus.emit(CameraSubjectChanged {
            before: b.camera_subject,
            after: a.camera_subject,
        });
    }
}

fn same(a: &Option<Arc<Session>>, b: &Option<Arc<Session>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (a, b) => a.is_none() && b.is_none(),
    }
}

struct PollState {
    last_status: AppStatus,
    last_session: Option<Arc<Session>>,
    roots: RootsCache,
    state: RobloxState,
}

/// Shared across threads: `poll()` serializes on an internal lock (run() is such a loop); the getters read atomics.
/// Events per poll, in order: StatusChanged, RobloxOpened/RobloxClosed, SessionReady/SessionLost, then state changes.
/// Handlers run on the polling thread, under that lock, so a handler must not call poll().
pub struct RobloxWatcher {
    session: Box<dyn Fn() -> Option<Arc<Session>> + Send + Sync>,
    status: Box<dyn Fn() -> AppStatus + Send + Sync>,
    detail: Box<dyn Fn() -> String + Send + Sync>,
    bus: EventBus,
    groups: RobloxGroups,
    poll_state: Mutex<PollState>,
    chat: AtomicI8,
    character: AtomicUsize,
    camera: AtomicUsize,
}

impl RobloxWatcher {
    /// `groups` lists the key groups the game wants polled (e.g. CHAT_KEYS, or several joined), not its whole
    /// required list: a group (chat/character/camera) is polled only when ALL its CHAT_KEYS/CHARACTER_KEYS/CAMERA_KEYS
    /// are in `groups`, so a game pays only for what it consumes.
    pub fn new(
        session: impl Fn() -> Option<Arc<Session>> + Send + Sync + 'static,
        status: impl Fn() -> AppStatus + Send + Sync + 'static,
        detail: impl Fn() -> String + Send + Sync + 'static,
        bus: EventBus,
        groups: &[&str],
    ) -> Self {
        Self {
            session: Box::new(session),
            status: Box::new(status),
            detail: Box::new(detail),
            bus,
            groups: groups_for(groups),
            poll_state: Mutex::new(PollState {
                last_status: AppStatus::RobloxClosed,
                last_session: None,
                roots: RootsCache::default(),
                state: RobloxState::default(),
            }),
            chat: AtomicI8::new(-1),
            character: AtomicUsize::new(0),
            camera: AtomicUsize::new(0),
        }
    }

    pub fn for_updater(u: &Arc<Updater>, bus: EventBus, groups: &[&str]) -> Self {
        let (a, b, c) = (u.clone(), u.clone(), u.clone());
        Self::new(move || a.session(), move || b.status(), move || c.detail(), bus, groups)
    }

    pub fn poll(&self) {
        let mut ps = self.poll_state.lock().unwrap();
        let sess = (self.session)();
        let st = (self.status)();
        if st != ps.last_status {
            self.bus.emit(StatusChanged {
                from: ps.last_status,
                to: st,
                detail: (self.detail)(),
            });
            if ps.last_status == AppStatus::RobloxClosed {
                self.bus.emit(RobloxOpened);
            }
            if st == AppStatus::RobloxClosed {
                self.bus.emit(RobloxClosed);
            }
            ps.last_status = st;
        }
        let changed = !same(&sess, &ps.last_session);
        if let Some(s) = &sess
            && changed
        {
            self.bus.emit(SessionReady { session: s.clone() });
        }
        if sess.is_none() && ps.last_session.is_some() {
            self.bus.emit(SessionLost);
        }
        if changed {
            ps.roots = RootsCache::default(); // dm differs or is re-read below
            ps.last_session = sess.clone();
        }
        let mut now = RobloxState::default();
        // Offsets that lack an SDK key leave the state default.
        if let Some(s) = &sess
            && s.offsets.has_all(&SDK_KEYS)
        {
            let dm = data_model(&s.proc, &s.offsets);
            update_roots(&mut ps.roots, &s.proc, &s.offsets, dm, Instant::now());
            now = read_roblox_state(&s.proc, &s.offsets, &ps.roots.roots, self.groups);
        }
        let before = std::mem::replace(&mut ps.state, now);
        self.chat.store(now.chat_focused.map_or(-1, i8::from), Relaxed);
        self.character.store(now.character, Relaxed);
        self.camera.store(now.camera_subject, Relaxed);
        emit_changes(&before, &now, &self.bus);
    }

    pub fn run(&self, stop: &AtomicBool, every: Duration) {
        while !stop.load(Relaxed) {
            self.poll();
            std::thread::sleep(every);
        }
    }

    pub fn chat_focused(&self) -> Option<bool> {
        let c = self.chat.load(Relaxed);
        (c >= 0).then_some(c != 0)
    }
    pub fn character(&self) -> usize {
        self.character.load(Relaxed)
    }
    pub fn camera_subject(&self) -> usize {
        self.camera.load(Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeMemory, fake_instance_offsets};

    fn ev_offs() -> Offsets {
        Offsets::from_pairs(&[
            ("UserInputService.WindowInputState", 0x10),
            ("WindowInputState.CurrentTextBox", 0x20),
            ("Player.LocalPlayer", 0x30),
            ("Player.ModelInstance", 0x40),
            ("DataModel.Workspace", 0x50),
            ("Workspace.CurrentCamera", 0x60),
            ("Camera.CameraSubject", 0x70),
        ])
    }

    struct World {
        m: FakeMemory,
        r: RobloxRoots,
    }

    impl World {
        fn new(text_box: usize, chr: usize, subject: usize) -> Self {
            let mut m = FakeMemory::default();
            let r = RobloxRoots {
                data_model: m.alloc(0x80),
                players: m.alloc(0x80),
                user_input_service: m.alloc(0x80),
            };
            let (lp, wis, ws, cam) = (m.alloc(0x80), m.alloc(0x80), m.alloc(0x80), m.alloc(0x80));
            m.put_ptr(r.user_input_service + 0x10, wis);
            m.put_ptr(wis + 0x20, text_box);
            m.put_ptr(r.players + 0x30, lp);
            m.put_ptr(lp + 0x40, chr);
            m.put_ptr(r.data_model + 0x50, ws);
            m.put_ptr(ws + 0x60, cam);
            m.put_ptr(cam + 0x70, subject);
            Self { m, r }
        }
    }

    fn all_keys() -> Vec<&'static str> {
        CHAT_KEYS
            .iter()
            .chain(&CHARACTER_KEYS)
            .chain(&CAMERA_KEYS)
            .copied()
            .collect()
    }

    fn log() -> Arc<Mutex<Vec<String>>> {
        Arc::default()
    }

    fn take(got: &Mutex<Vec<String>>) -> Vec<String> {
        std::mem::take(&mut *got.lock().unwrap())
    }

    #[test]
    fn events_read_state_chat_focused_and_not() {
        let (on, off, o) = (World::new(0x5000, 0x111, 0x222), World::new(0, 0x111, 0x222), ev_offs());
        let a = read_roblox_state(&on.m, &o, &on.r, RobloxGroups::default());
        assert_eq!(a.chat_focused, Some(true));
        assert_eq!(a.character, 0x111);
        assert_eq!(a.camera_subject, 0x222);
        assert_eq!(
            read_roblox_state(&off.m, &o, &off.r, RobloxGroups::default()).chat_focused,
            Some(false)
        );
    }

    #[test]
    fn events_read_state_chat_unreadable_is_none() {
        let (mut w, o) = (World::new(0x5000, 1, 2), ev_offs());
        let none = RobloxRoots {
            user_input_service: 0,
            ..w.r
        };
        assert!(
            read_roblox_state(&w.m, &o, &none, RobloxGroups::default())
                .chat_focused
                .is_none()
        );
        w.m.put_ptr(w.r.user_input_service + 0x10, 0); // WindowInputState null
        assert!(
            read_roblox_state(&w.m, &o, &w.r, RobloxGroups::default())
                .chat_focused
                .is_none()
        );
        w.m.put_ptr(w.r.user_input_service + 0x10, 0xdead0000); // unreadable
        assert!(
            read_roblox_state(&w.m, &o, &w.r, RobloxGroups::default())
                .chat_focused
                .is_none()
        );
    }

    #[test]
    fn events_read_state_skips_groups_without_keys() {
        let (w, mut o) = (World::new(0x5000, 0x111, 0x222), ev_offs());
        o.values.remove("Player.ModelInstance");
        let s = read_roblox_state(&w.m, &o, &w.r, RobloxGroups::default());
        assert_eq!(s.character, 0);
        assert_eq!(s.camera_subject, 0x222);
    }

    #[test]
    fn events_emit_changes_only_what_changed() {
        let (bus, got) = (EventBus::new(), log());
        let g = got.clone();
        let _a = bus.on(move |e: &ChatFocusChanged| {
            let s = match e.focused {
                Some(true) => "chat on",
                Some(false) => "chat off",
                None => "chat ?",
            };
            g.lock().unwrap().push(s.into());
        });
        let g = got.clone();
        let _b = bus.on(move |e: &CharacterChanged| g.lock().unwrap().push(format!("char {}>{}", e.before, e.after)));
        let st = |chat, character| RobloxState {
            chat_focused: chat,
            character,
            camera_subject: 0,
        };
        emit_changes(&st(Some(false), 0x10), &st(Some(true), 0x10), &bus);
        emit_changes(&st(Some(true), 0x10), &st(None, 0x20), &bus);
        emit_changes(&st(None, 0x20), &st(None, 0x20), &bus);
        assert_eq!(take(&got), ["chat on", "chat ?", "char 16>32"]);
    }

    #[test]
    fn events_watcher_order() {
        let (bus, got) = (EventBus::new(), log());
        let push = |s: &'static str| {
            let g = got.clone();
            move || g.lock().unwrap().push(s.to_owned())
        };
        let (p1, p2, p3, p4, p5) = (
            push("status"),
            push("opened"),
            push("closed"),
            push("ready"),
            push("lost"),
        );
        let _s1 = bus.on(move |_: &StatusChanged| p1());
        let _s2 = bus.on(move |_: &RobloxOpened| p2());
        let _s3 = bus.on(move |_: &RobloxClosed| p3());
        let _s4 = bus.on(move |_: &SessionReady| p4());
        let _s5 = bus.on(move |_: &SessionLost| p5());

        let sess: Arc<Mutex<Option<Arc<Session>>>> = Arc::default();
        let st = Arc::new(Mutex::new(AppStatus::RobloxClosed));
        let (s, t) = (sess.clone(), st.clone());
        let w = RobloxWatcher::new(
            move || s.lock().unwrap().clone(),
            move || *t.lock().unwrap(),
            || "d".into(),
            bus.clone(),
            &all_keys(),
        );
        w.poll();
        assert!(take(&got).is_empty());
        *st.lock().unwrap() = AppStatus::Validating;
        w.poll();
        assert_eq!(take(&got), ["status", "opened"]);
        *st.lock().unwrap() = AppStatus::Ready;
        *sess.lock().unwrap() = Some(Arc::new(Session::default()));
        w.poll();
        assert_eq!(take(&got), ["status", "ready"]);
        w.poll();
        assert!(take(&got).is_empty());
        *sess.lock().unwrap() = None;
        *st.lock().unwrap() = AppStatus::RobloxClosed;
        w.poll();
        assert_eq!(take(&got), ["status", "closed", "lost"]);
        assert!(w.chat_focused().is_none());
        assert_eq!(w.character(), 0);
    }

    #[test]
    fn events_watcher_session_swap_emits_ready_only() {
        let (bus, got) = (EventBus::new(), log());
        let g = got.clone();
        let _a = bus.on(move |_: &SessionReady| g.lock().unwrap().push("ready".into()));
        let g = got.clone();
        let _b = bus.on(move |_: &SessionLost| g.lock().unwrap().push("lost".into()));
        let cur = Arc::new(Mutex::new(Some(Arc::new(Session::default()))));
        let c = cur.clone();
        let w = RobloxWatcher::new(
            move || c.lock().unwrap().clone(),
            || AppStatus::Ready,
            String::new,
            bus.clone(),
            &all_keys(),
        );
        w.poll();
        take(&got);
        *cur.lock().unwrap() = Some(Arc::new(Session::default()));
        w.poll();
        assert_eq!(take(&got), ["ready"]);
    }

    #[test]
    fn events_watcher_missing_offsets_do_not_panic() {
        let sess = Arc::new(Session::default()); // empty offsets: FakeDataModel.Pointer missing
        let w = RobloxWatcher::new(
            move || Some(sess.clone()),
            || AppStatus::Ready,
            String::new,
            EventBus::new(),
            &all_keys(),
        );
        w.poll();
        assert!(w.chat_focused().is_none());
        assert_eq!(w.character(), 0);
        assert_eq!(w.camera_subject(), 0);
    }

    #[test]
    fn events_update_roots_throttles_missing_children() {
        let mut m = FakeMemory::default();
        let o = fake_instance_offsets();
        let mut node = |cls: &str, kids: &[usize]| {
            let (inst, desc) = (m.alloc(0x80), m.alloc(0x20));
            m.put_ptr(inst + 0x18, desc);
            let s = m.str(cls);
            m.put_ptr(desc + 0x8, s);
            let s = m.str(cls);
            m.put_ptr(inst + 0x20, s);
            if !kids.is_empty() {
                let (vec, arr) = (m.alloc(0x20), m.alloc(16 * kids.len()));
                m.put_ptr(vec, arr);
                m.put_ptr(vec + 8, arr + 16 * kids.len());
                for (i, k) in kids.iter().enumerate() {
                    m.put_ptr(arr + 16 * i, *k);
                }
                m.put_ptr(inst + 0x30, vec);
            }
            inst
        };
        let (players, uis) = (node("Players", &[]), node("UserInputService", &[]));
        let (dm1, dm2) = (node("DataModel", &[players]), node("DataModel", &[players, uis]));
        let mut c = RootsCache::default();
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        assert!(update_roots(&mut c, &m, &o, dm1, t0)); // dm changed: resolves at once
        assert_eq!(c.roots.user_input_service, 0);
        assert!(!update_roots(&mut c, &m, &o, dm1, t0 + ms(500))); // throttled
        assert!(update_roots(&mut c, &m, &o, dm1, t0 + ms(1100))); // retry window passed
        assert!(update_roots(&mut c, &m, &o, dm2, t0 + ms(1200))); // dm change ignores throttle
        assert_eq!(c.roots.user_input_service, uis);
        assert!(!update_roots(&mut c, &m, &o, dm2, t0 + ms(5000))); // complete: no more walks
    }

    #[test]
    fn events_watcher_polls_only_required_groups() {
        let w = World::new(0x5000, 0x111, 0x222);
        let s = read_roblox_state(
            &w.m,
            &ev_offs(),
            &w.r,
            RobloxGroups {
                chat: true,
                character: false,
                camera: false,
            },
        );
        assert!(s.chat_focused.is_some());
        assert_eq!(s.character, 0);
        assert_eq!(s.camera_subject, 0);
        let g = groups_for(&CHAT_KEYS);
        assert!(g.chat && !g.character && !g.camera);
        let mut shuffled = all_keys();
        shuffled.reverse();
        shuffled.push(CHAT_KEYS[0]); // duplicate
        assert_eq!(groups_for(&shuffled), RobloxGroups::default());
        let mut no_cam = all_keys();
        no_cam.pop(); // drops "Camera.CameraSubject"
        let g = groups_for(&no_cam);
        assert!(g.chat && g.character && !g.camera);
        let g = groups_for(&[]);
        assert!(!g.chat && !g.character && !g.camera);
    }
}
