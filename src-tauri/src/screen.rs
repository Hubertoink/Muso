//! Native display and input handling for the controller and TV.
#[cfg(windows)]
pub fn watch(window: tauri::WebviewWindow) -> tauri::Result<()> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use tauri::WindowEvent;
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        Graphics::Gdi::SC_SCREENSAVE,
        System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED},
        UI::{Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::WM_SYSCOMMAND},
    };

    unsafe extern "system" fn screensaver(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM, _: usize, _: usize) -> LRESULT {
        if msg == WM_SYSCOMMAND && (w as u32 & 0xfff0) == SC_SCREENSAVE {
            return 0;
        }
        unsafe { DefSubclassProc(hwnd, msg, w, l) }
    }

    let hwnd = window.hwnd()?.0 as usize;
    let player = window.clone();
    let active = AtomicBool::new(false);
    // Native window events and the subclass run on the owning UI thread. Windows
    // tracks the idle request per thread, and releases it if the process exits.
    let sync = move |visible_fullscreen: bool| {
        if active.load(Ordering::Relaxed) == visible_fullscreen { return; }
        unsafe {
            if visible_fullscreen {
                if SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED) == 0 { return; }
                SetWindowSubclass(hwnd as HWND, Some(screensaver), 1, 0);
            } else {
                RemoveWindowSubclass(hwnd as HWND, Some(screensaver), 1);
                SetThreadExecutionState(ES_CONTINUOUS);
            }
        }
        active.store(visible_fullscreen, Ordering::Relaxed);
    };
    let visible_fullscreen = move || player.is_fullscreen().unwrap_or(false)
        && player.is_visible().unwrap_or(false) && !player.is_minimized().unwrap_or(true);
    sync(visible_fullscreen());
    window.on_window_event(move |event| match event {
        WindowEvent::Resized(_) | WindowEvent::Focused(_) => sync(visible_fullscreen()),
        WindowEvent::Destroyed => sync(false),
        _ => {}
    });
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn focus_on_touch(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    use gtk::{gdk::{EventMask, EventType, InputSource}, glib::Propagation, prelude::*};
    use std::cell::Cell;
    window.with_webview(|platform| {
        let view = platform.inner();
        view.add_events(EventMask::TOUCH_MASK);
        let restore_tooltips = Cell::new(false);
        view.connect_event(move |view, event| {
            if event.event_type() == EventType::TouchBegin {
                if view.has_tooltip() {
                    restore_tooltips.set(true);
                    view.set_has_tooltip(false);
                }
                if let Some(window) = view.toplevel().and_then(|w| w.downcast::<gtk::Window>().ok()) {
                    if !window.is_active() { window.present_with_time(event.time()); }
                }
                if !view.has_focus() { view.grab_focus(); }
            } else if restore_tooltips.get() && event.event_type() == EventType::MotionNotify
                && !event.is_pointer_emulated() && event.source_device().is_some_and(|device|
                    matches!(device.source(), InputSource::Mouse | InputSource::Touchpad | InputSource::Trackpoint)) {
                view.set_has_tooltip(true);
                restore_tooltips.set(false);
            }
            // Let WebKit handle this same first contact as a tap or swipe.
            Propagation::Proceed
        });
    })
}

#[cfg(target_os = "linux")]
pub fn watch(window: tauri::WebviewWindow) -> tauri::Result<()> {
    use gtk::{gdk::WindowState, glib::Propagation, prelude::*, ApplicationInhibitFlags};
    use std::{cell::Cell, rc::Rc};
    let window = window.gtk_window()?;
    let cookie = Rc::new(Cell::new(0));
    let pending = cookie.clone();
    window.connect_window_state_event(move |win, event| {
        if let Some(app) = win.application() {
            let state = event.new_window_state();
            let fullscreen = state.contains(WindowState::FULLSCREEN)
                && !state.intersects(WindowState::ICONIFIED | WindowState::WITHDRAWN);
            if fullscreen && pending.get() == 0 {
                pending.set(app.inhibit(Some(win), ApplicationInhibitFlags::IDLE | ApplicationInhibitFlags::SUSPEND, Some("Muso is in fullscreen")));
            } else if !fullscreen && pending.get() != 0 {
                app.uninhibit(pending.replace(0));
            }
        }
        Propagation::Proceed
    });
    window.connect_unmap(move |win| {
        if let Some(app) = win.application() {
            if cookie.get() != 0 { app.uninhibit(cookie.replace(0)); }
        }
    });
    Ok(())
}
