//! A small window built with the SDK's Dashboard: one of each row kind on two pages, plus the status pill. Dummy
//! data, no Roblox. `--self-test` closes the window after about a second.
use std::cell::{Cell, RefCell};
use std::time::Duration;

use handy_sdk::ui::Tone;
use handy_sdk::ui::dashboard::{
    Dashboard, Span, Spec, TreeItem, check, note, number, page, rich_value, section, text, tree, value,
};
use handy_sdk::ui::overlay::StatusOverlay;
use handy_sdk::ui::theme::rgb;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, SW_SHOWDEFAULT, WM_CLOSE};

const ON: i32 = 2;
const TICKS: i32 = 3;
const CATCH: i32 = 4;
const DELAY: i32 = 5;
const FIND: i32 = 6;
const HINT: i32 = 7;
const PICK: i32 = 8;

fn items(checked: &[bool], find: &str) -> Vec<TreeItem> {
    let names = ["Coin", "Gem", "Key"];
    let mut out = vec![TreeItem {
        key: "W".into(),
        text: "Workspace".into(),
        checkbox: false,
        open: true,
        ..Default::default()
    }];
    for (i, name) in names.iter().enumerate() {
        if name.to_lowercase().contains(&find.to_lowercase()) {
            out.push(TreeItem {
                key: format!("W/{name}"),
                parent: Some(0),
                text: (*name).into(),
                checked: checked[i],
                ..Default::default()
            });
        }
    }
    out
}

fn main() {
    let self_test = std::env::args().any(|a| a == "--self-test");
    let on = Cell::new(true);
    let ticks = Cell::new(0u32);
    let picked = RefCell::new(vec![false, true, false]);
    let find = RefCell::new(String::new());
    let pill = StatusOverlay::new();
    pill.create();

    let d = Dashboard::new(Spec {
        title: "handy's sdk demo".into(),
        badge: handy_sdk::app::self_update::badge_text(
            "1.2.3",
            Some(handy_sdk::app::self_update::UpdateStatus::Latest),
        ),
        window_class: "handys-sdk-demo-dashboard".into(),
        rows: vec![
            page("Overview"),
            section("Demo"),
            check(ON, "Demo switch\tpress F1"),
            value(TICKS, "Ticks"),
            rich_value(CATCH, "Last catch").lines(2),
            page("Settings"),
            section("Timings"),
            number(DELAY, "Delay (ms)", 0, 1000),
            note(HINT),
            section("Pick"),
            text(FIND, "Find"),
            tree(PICK, 6),
        ],
        hint_row: Some(HINT),
        ..Default::default()
    });
    d.on_tick(|ui| {
        let n = ticks.get() + 1;
        ticks.set(n);
        if n == 1 {
            ui.set_number(DELAY, 250);
            ui.set_tree(PICK, &items(&picked.borrow(), ""));
            if self_test {
                let h = ui.hwnd().0 as isize; // an HWND is a plain handle, fine to post to from another thread
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(1));
                    let _ = unsafe { PostMessageW(Some(HWND(h as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0)) };
                });
            }
        }
        let tone = if on.get() { Tone::Good } else { Tone::Neutral };
        ui.set_status(
            tone,
            if on.get() { "Ready" } else { "Switched off" },
            "dummy data, no Roblox",
        );
        ui.set_check(ON, on.get());
        ui.set_text(TICKS, &n.to_string());
        ui.set_spans(
            CATCH,
            &[
                Span {
                    text: "Carrot ".into(),
                    colors: vec![rgb(255, 162, 0)],
                    bold: true,
                    ..Default::default()
                },
                Span {
                    text: "Mackerel".into(),
                    colors: vec![rgb(142, 187, 191), rgb(0, 0, 255)],
                    ..Default::default()
                },
                Span::plain("\n3.9 kg"),
            ],
        );
        pill.set(if on.get() { "Demo on" } else { "Demo off" }, on.get());
    });
    d.on_check(|_, id, checked| {
        if id == ON {
            on.set(checked);
        }
    });
    d.on_text(|ui, _, s| {
        s.clone_into(&mut find.borrow_mut());
        ui.set_tree(PICK, &items(&picked.borrow(), s));
    });
    d.on_tree_check(|ui, _, index, checked| {
        let shown = items(&picked.borrow(), &find.borrow());
        let names = ["W/Coin", "W/Gem", "W/Key"];
        if let Some(i) = shown.get(index).and_then(|it| names.iter().position(|n| *n == it.key)) {
            picked.borrow_mut()[i] = checked;
        }
        ui.set_tree(PICK, &items(&picked.borrow(), &find.borrow()));
    });
    let rc = d.run(SW_SHOWDEFAULT);
    drop(d);
    drop(pill);
    std::process::exit(rc);
}
