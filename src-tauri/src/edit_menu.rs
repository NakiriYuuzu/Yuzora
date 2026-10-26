//! macOS Edit menu whose Undo/Redo reach CodeMirror and whose Select All can
//! be held back.
//!
//! Tauri's default Edit items send `undo:`/`redo:` straight to WebKit and
//! swallow ⌘Z/⇧⌘Z before the page sees a key press. WebKit only knows the
//! edits it recorded in the current DOM, so CodeMirror's own history (kept
//! across a tab switch, or changes it applies itself such as Tab indent) could
//! never be undone. Select All swallows ⌘A the same way, so no page handler
//! could keep it from highlighting the whole page behind a dialog. These items
//! ask the frontend first: it runs the focused editor's undo/redo or drops a
//! Select All that would only highlight the page, and otherwise hands the
//! action back to `edit_history_native`, which sends the selector the default
//! items sent.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EditMenuAction {
    Undo,
    Redo,
    SelectAll,
}

#[cfg(any(target_os = "macos", test))]
const UNDO_ID: &str = "yuzora.edit.undo";
#[cfg(any(target_os = "macos", test))]
const REDO_ID: &str = "yuzora.edit.redo";
#[cfg(any(target_os = "macos", test))]
const SELECT_ALL_ID: &str = "yuzora.edit.selectAll";
#[cfg(target_os = "macos")]
const EVENT: &str = "edit:history";

/// Predefined Edit items replaced in place: (text without mnemonic, id, accelerator).
#[cfg(any(target_os = "macos", test))]
const REPLACED_ITEMS: [(&str, &str, &str); 3] = [
    ("Undo", UNDO_ID, "CmdOrCtrl+Z"),
    ("Redo", REDO_ID, "CmdOrCtrl+Shift+Z"),
    ("Select All", SELECT_ALL_ID, "CmdOrCtrl+A"),
];

#[cfg(any(target_os = "macos", test))]
fn action_for_menu_id(id: &str) -> Option<EditMenuAction> {
    match id {
        UNDO_ID => Some(EditMenuAction::Undo),
        REDO_ID => Some(EditMenuAction::Redo),
        SELECT_ALL_ID => Some(EditMenuAction::SelectAll),
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

/// Tauri's default menu with its predefined Undo/Redo/Select All replaced in place.
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
        for (text, id, accelerator) in REPLACED_ITEMS {
            // muda's default "Select &All" keeps its mnemonic until AppKit strips it.
            let position = edit.items()?.iter().position(|item| {
                matches!(item, MenuItemKind::Predefined(predefined)
                    if predefined.text().is_ok_and(|label| label.replace('&', "") == text))
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

/// Sends an Edit menu action the frontend left alone to the key window's responder chain.
#[tauri::command]
pub fn edit_history_native(app: tauri::AppHandle, action: EditMenuAction) -> Result<(), String> {
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
fn send_to_first_responder(action: EditMenuAction) {
    use objc2::{sel, MainThreadMarker};
    use objc2_app_kit::NSApplication;
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let selector = match action {
        EditMenuAction::Undo => sel!(undo:),
        EditMenuAction::Redo => sel!(redo:),
        EditMenuAction::SelectAll => sel!(selectAll:),
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
        assert_eq!(action_for_menu_id(UNDO_ID), Some(EditMenuAction::Undo));
        assert_eq!(action_for_menu_id(REDO_ID), Some(EditMenuAction::Redo));
        assert_eq!(
            action_for_menu_id(SELECT_ALL_ID),
            Some(EditMenuAction::SelectAll)
        );
        assert_eq!(action_for_menu_id("undo"), None);
        for (_, id, _) in REPLACED_ITEMS {
            assert!(action_for_menu_id(id).is_some(), "{id} has no action");
        }
    }

    #[test]
    fn actions_cross_ipc_as_the_frontend_names() {
        assert_eq!(
            serde_json::to_string(&EditMenuAction::Undo).unwrap(),
            "\"undo\""
        );
        assert_eq!(
            serde_json::from_str::<EditMenuAction>("\"redo\"").unwrap(),
            EditMenuAction::Redo
        );
        assert_eq!(
            serde_json::to_string(&EditMenuAction::SelectAll).unwrap(),
            "\"selectAll\""
        );
    }
}
