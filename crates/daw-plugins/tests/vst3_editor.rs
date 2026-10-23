//! A VST3 plugin's window (this repo's test plugin), in a host process:
//! opening, resizing either way, closing, and knob moves in it.

#![cfg(windows)]
#![allow(clippy::unwrap_used, reason = "tests fail on error")]
#![allow(unsafe_code, reason = "resizing the host's window from outside")]

mod common;

use common::windows::{click, client_size, view_of, wait_for, window_rect, windows_of};
use common::{GAIN_CID, install};
use daw_plugins::block::{Context, DawBlock, Layout, ParamChange};
use daw_plugins::client::{HostCommand, HostProcess, next_instance};
use daw_plugins::control::{HostEvent, PluginSpec};
use std::time::{Duration, Instant};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SWP_NOMOVE, SWP_NOZORDER, SetWindowPos, WM_CLOSE, WM_LBUTTONDOWN, WM_MBUTTONDOWN,
    WM_RBUTTONDOWN,
};

fn host_command() -> HostCommand {
    HostCommand {
        program: env!("CARGO_BIN_EXE_daw-plugin-host").into(),
        args: Vec::new(),
    }
}

/// Test Gain, created and processing.
fn gain(name: &str) -> (HostProcess, u64, DawBlock) {
    let (_, bundle) = install(name);
    let mut host = HostProcess::spawn(&host_command()).unwrap();
    let id = next_instance();
    let spec = PluginSpec::Vst3 {
        bundle,
        cid: GAIN_CID.into(),
        name: "Test Gain".into(),
        vendor: "DAW Tests".into(),
    };
    host.create(id, spec, None).unwrap();
    let layout = Layout {
        max_frames: 256,
        inputs: 2,
        outputs: 2,
    };
    let block = host.activate(id, layout, 48_000.).unwrap();
    (host, id, block)
}

/// Collects the host's events until there are `count`.
fn events(host: &mut HostProcess, count: usize) -> Vec<HostEvent> {
    let mut got = Vec::new();
    wait_for("events", || {
        got.extend(host.take_events());
        got.len() >= count
    });
    got
}

#[test]
fn a_plugin_window_opens_resizes_closes_and_reopens_where_it_was() {
    let (mut host, id, _block) = gain("editor-window");
    let pid = host.pid();
    assert_eq!(
        host.open_editor(id, "Test Gain — Drums").unwrap(),
        (400, 300)
    );
    let windows = windows_of(pid);
    assert_eq!(windows.len(), 1);
    let (window, title) = windows[0].clone();
    assert_eq!(title, "Test Gain — Drums");
    let view = view_of(window);
    assert_eq!(client_size(window), (400, 300));
    assert_eq!(client_size(view), (400, 300));

    // Asking again brings the same window forward (and renames it).
    host.open_editor(id, "Test Gain — Bass").unwrap();
    let windows = windows_of(pid);
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0].1, "Test Gain — Bass");

    // The plugin asks for another size.
    click(view, WM_RBUTTONDOWN);
    wait_for("the plugin's resize", || {
        client_size(window) == (600, 450) && client_size(view) == (600, 450)
    });

    // The window is resized from outside: the plugin follows.
    let outer = window_rect(window);
    let border = (
        outer.right - outer.left - 600,
        outer.bottom - outer.top - 450,
    );
    unsafe {
        SetWindowPos(
            window,
            std::ptr::null_mut(),
            0,
            0,
            800 + border.0,
            500 + border.1,
            SWP_NOMOVE | SWP_NOZORDER,
        )
    };
    wait_for("the view to follow", || client_size(view) == (800, 500));

    // The user closes it: the DAW hears, and it reopens where it was.
    let place = window_rect(window);
    click(window, WM_CLOSE);
    assert_eq!(
        events(&mut host, 1),
        [HostEvent::EditorClosed { instance: id }]
    );
    wait_for("the window to go", || windows_of(pid).is_empty());
    host.open_editor(id, "Test Gain — Bass").unwrap();
    let again = windows_of(pid)[0].0;
    let reopened = window_rect(again);
    assert_eq!((reopened.left, reopened.top), (place.left, place.top));

    host.close_editor(id).unwrap();
    assert!(windows_of(pid).is_empty(), "closed by the DAW");
}

#[test]
fn knob_moves_reach_the_daw_and_the_sound_and_the_window_follows_the_song() {
    let (mut host, id, mut block) = gain("editor-knobs");
    host.open_editor(id, "Test Gain").unwrap();
    let view = view_of(windows_of(host.pid())[0].0);
    let mut play = |params: &[ParamChange]| {
        let input = vec![0.5; 64];
        let (mut l, mut r) = (vec![0.; 64], vec![0.; 64]);
        block.process(
            &[&input, &input],
            params,
            Context::default(),
            &mut [&mut l, &mut r],
            Some(Instant::now() + Duration::from_secs(2)),
        );
        l[10]
    };

    // A gesture: begin, the value, end.
    click(view, WM_LBUTTONDOWN);
    assert_eq!(
        events(&mut host, 3),
        [
            HostEvent::EditBegin {
                instance: id,
                id: 0
            },
            HostEvent::EditValue {
                instance: id,
                id: 0,
                value: 0.75
            },
            HostEvent::EditEnd {
                instance: id,
                id: 0
            },
        ]
    );
    assert_eq!(play(&[]), 0.75, "the processor heard it: gain 1.5");

    // A move without a gesture (as a mouse wheel makes).
    click(view, WM_MBUTTONDOWN);
    assert_eq!(
        events(&mut host, 1),
        [HostEvent::EditValue {
            instance: id,
            id: 7,
            value: 0.3
        }]
    );

    // The song moves Gain: the plugin's controller (its window) follows.
    let song = [ParamChange {
        id: 0,
        offset: 0,
        value: 0.25,
    }];
    assert_eq!(play(&song), 0.25);
    wait_for("the window to follow", || {
        host.params(id).unwrap()[0].value == 0.25
    });
}
