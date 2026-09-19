//! Window activation is separate from WebView keyboard focus on Windows.
//! In particular, returning through the taskbar can activate the top-level HWND
//! without a WebView GotFocus event (and Tao's is_focused remains false).

#[tauri::command]
pub fn workbench_is_window_active(window: tauri::Window) -> Result<bool, String> {
    #[cfg(windows)]
    {
        let hwnd = window.hwnd().map_err(|error| error.to_string())?;
        // Unlike GetFocus/Tao's focus state, this includes activation while a
        // child WebView owns, or has not yet recovered, keyboard focus.
        Ok(unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() == hwnd.0 })
    }
    #[cfg(not(windows))]
    window.is_focused().map_err(|error| error.to_string())
}

#[cfg(windows)]
pub fn install(window: &tauri::WebviewWindow) -> Result<(), String> {
    use tauri::Emitter;
    let hwnd = window.hwnd().map_err(|error| error.to_string())?;
    let window = window.clone();
    // setup() runs on the window's UI thread, as required by SetWindowSubclass.
    native::install(hwnd.0, move |active| {
        let _ = window.emit("workbench:window-activation", active);
    })
}

#[cfg(windows)]
mod native {
    use std::sync::Arc;
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        UI::{
            Shell::{DefSubclassProc, GetWindowSubclass, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::{WA_INACTIVE, WM_ACTIVATE, WM_NCDESTROY},
        },
    };

    const SUBCLASS_ID: usize = 0x59555a46;
    struct Listener(Arc<dyn Fn(bool) + Send + Sync>);

    pub(super) fn install(
        hwnd: HWND,
        notify: impl Fn(bool) + Send + Sync + 'static,
    ) -> Result<(), String> {
        unsafe {
            let mut existing = 0;
            if GetWindowSubclass(hwnd, Some(activation_proc), SUBCLASS_ID, &mut existing) != 0 {
                return Ok(());
            }
            let listener = Box::into_raw(Box::new(Listener(Arc::new(notify))));
            if SetWindowSubclass(hwnd, Some(activation_proc), SUBCLASS_ID, listener as usize) == 0 {
                drop(Box::from_raw(listener));
                return Err("failed to observe Windows window activation".into());
            }
        }
        Ok(())
    }

    unsafe extern "system" fn activation_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        id: usize,
        data: usize,
    ) -> LRESULT {
        if message == WM_NCDESTROY {
            RemoveWindowSubclass(hwnd, Some(activation_proc), id);
            let listener = Box::from_raw(data as *mut Listener);
            let result = DefSubclassProc(hwnd, message, wparam, lparam);
            drop(listener);
            return result;
        }
        // Hold a separate reference across DefSubclassProc: default processing
        // can reenter this callback, including destroying the window.
        let notification = (message == WM_ACTIVATE).then(|| {
            (
                Arc::clone(&(*(data as *const Listener)).0),
                (wparam & 0xffff) != WA_INACTIVE as usize,
            )
        });
        let result = DefSubclassProc(hwnd, message, wparam, lparam);
        if let Some((notify, active)) = notification {
            notify(active);
        }
        result
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Mutex,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, SendMessageW, HWND_MESSAGE, WA_ACTIVE, WA_CLICKACTIVE,
        };

        #[test]
        fn native_activation_is_independent_of_webview_focus_and_releases_listener() {
            let events = Arc::new(Mutex::new(Vec::new()));
            let dropped = Arc::new(AtomicBool::new(false));
            struct DropSignal(Arc<AtomicBool>);
            impl Drop for DropSignal {
                fn drop(&mut self) {
                    self.0.store(true, Ordering::SeqCst);
                }
            }
            // A hidden message-only HWND exercises the actual subclass lifecycle
            // without activating a desktop window or requiring WebView2.
            let class = [83u16, 84, 65, 84, 73, 67, 0]; // STATIC
            unsafe {
                let hwnd = CreateWindowExW(
                    0,
                    class.as_ptr(),
                    std::ptr::null(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    HWND_MESSAGE,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );
                assert!(!hwnd.is_null());
                let received = events.clone();
                let signal = DropSignal(dropped.clone());
                install(hwnd, move |active| {
                    let _keep_alive = &signal;
                    received.lock().unwrap().push(active);
                })
                .unwrap();
                install(hwnd, |_| panic!("must not replace the installed listener")).unwrap();
                SendMessageW(hwnd, WM_ACTIVATE, WA_ACTIVE as usize, 0);
                SendMessageW(hwnd, WM_ACTIVATE, WA_INACTIVE as usize | (1 << 16), 0);
                SendMessageW(hwnd, WM_ACTIVATE, WA_CLICKACTIVE as usize, 0);
                // DefSubclassProc can synchronously produce another activation
                // while moving focus. Verify the state transitions, not a
                // platform-dependent count of identical notifications.
                let mut transitions = events.lock().unwrap().clone();
                transitions.dedup();
                assert_eq!(transitions, [true, false, true]);
                assert!(!dropped.load(Ordering::SeqCst));
                assert_ne!(DestroyWindow(hwnd), 0);
                assert!(dropped.load(Ordering::SeqCst));
            }
        }
    }
}
