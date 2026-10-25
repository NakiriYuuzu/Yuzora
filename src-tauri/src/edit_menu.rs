//! macOS Edit menu whose Undo/Redo reach CodeMirror.
//!
//! Tauri's default Edit items send `undo:`/`redo:` straight to WebKit and
//! swallow ⌘Z/⇧⌘Z before the page sees a key press. WebKit only knows the
//! edits it recorded in the current DOM, so CodeMirror's own history (kept
//! across a tab switch, or changes it applies itself such as Tab indent) could
//! never be undone. These two items ask the frontend first: it runs the focused
//! editor's undo/redo, and otherwise hands the action back to
//! `edit_history_native`, which sends the selector the default items sent.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EditHistoryAction {
    Undo,
    Redo,
}

#[cfg(any(target_os = "macos", test))]
const UNDO_ID: &str = "yuzora.edit.undo";
#[cfg(any(target_os = "macos", test))]
const REDO_ID: &str = "yuzora.edit.redo";
#[cfg(target_os = "macos")]
const EVENT: &str = "edit:history";

#[cfg(any(target_os = "macos", test))]
fn action_for_menu_id(id: &str) -> Option<EditHistoryAction> {
    match id {
        UNDO_ID => Some(EditHistoryAction::Undo),
        REDO_ID => Some(EditHistoryAction::Redo),
        _ => None,
    }
}

/// Uses the Edit menu below on macOS; other platforms keep having no app menu.
pub fn install<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    #[cfg(target_os = "macos")]
    {
        builder.menu(app_menu).on_menu_event(forward_menu_event)
    }
    #[cfg(not(target_os = "macos"))]
    {
        builder
    }
}

/// Tauri's default menu with its predefined Undo/Redo replaced in place.
#[cfg(target_os = "macos")]
fn app_menu<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> tauri::Result<tauri::menu::Menu<R>> {
    use tauri::menu::{Menu, MenuItem, MenuItemKind};
    let menu = Menu::default(app)?;
    for item in menu.items()? {
        let MenuItemKind::Submenu(edit) = item else {
            continue;
        };
        if edit.text()? != "Edit" {
            continue;
        }
        for (text, id, accelerator) in [
            ("Undo", UNDO_ID, "CmdOrCtrl+Z"),
            ("Redo", REDO_ID, "CmdOrCtrl+Shift+Z"),
        ] {
            let position = edit.items()?.iter().position(|item| {
                matches!(item, MenuItemKind::Predefined(predefined)
                    if predefined.text().ok().as_deref() == Some(text))
            });
            // A changed default layout keeps its native item rather than losing it.
            if let Some(position) = position {
                edit.remove_at(position)?;
                let replacement = MenuItem::with_id(app, id, text, true, Some(accelerator))?;
                edit.insert(&replacement, position)?;
            }
        }
    }
    Ok(menu)
}

#[cfg(target_os = "macos")]
fn forward_menu_event<R: tauri::Runtime>(app: &tauri::AppHandle<R>, event: tauri::menu::MenuEvent) {
    use tauri::Emitter;
    if let Some(action) = action_for_menu_id(event.id().as_ref()) {
        let _ = app.emit_to("main", EVENT, action);
    }
}

/// Sends an Undo/Redo no editor took to the key window's responder chain.
#[tauri::command]
pub fn edit_history_native(app: tauri::AppHandle, action: EditHistoryAction) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        app.run_on_main_thread(move || send_to_first_responder(action))
            .map_err(|error| error.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, action);
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn send_to_first_responder(action: EditHistoryAction) {
    use objc2::{sel, MainThreadMarker};
    use objc2_app_kit::NSApplication;
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let selector = match action {
        EditHistoryAction::Undo => sel!(undo:),
        EditHistoryAction::Redo => sel!(redo:),
    };
    // SAFETY: AppKit is used on the main thread. A nil target walks the key
    // window's responder chain, exactly like the predefined menu items.
    unsafe {
        NSApplication::sharedApplication(mtm).sendAction_to_from(selector, None, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_replaced_items_map_to_an_action() {
        assert_eq!(action_for_menu_id(UNDO_ID), Some(EditHistoryAction::Undo));
        assert_eq!(action_for_menu_id(REDO_ID), Some(EditHistoryAction::Redo));
        assert_eq!(action_for_menu_id("undo"), None);
    }

    #[test]
    fn actions_cross_ipc_as_the_frontend_names() {
        assert_eq!(
            serde_json::to_string(&EditHistoryAction::Undo).unwrap(),
            "\"undo\""
        );
        assert_eq!(
            serde_json::from_str::<EditHistoryAction>("\"redo\"").unwrap(),
            EditHistoryAction::Redo
        );
    }
}
