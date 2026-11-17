//! yuzora(unsaved): a person's close or quit that would lose unsaved changes
//! opens Save / Don't Save / Cancel (`crate::yuzora::unsaved`) instead of
//! running. The answer runs it again.

use super::*;
use crate::yuzora::unsaved;

impl AppWindow {
    /// Whether `name` opened the dialog, or met one already open, instead of
    /// running. One dialog at a time. A second close or quit sent while it is
    /// open is dropped, not kept. After the answer only the dialog's own
    /// command runs, and the person sends the other again. A close key with
    /// no target never gets here then, because `run_key` closes the dialog,
    /// as Cancel does.
    pub(super) fn ask_unsaved(&mut self, name: &str, args: &Value, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let ready = update_ready;
        let Some(spec) = self.folders.contains_key(unsaved::MOD).then(|| unsaved::dialog(name, args, &self.model, ready, cx)).flatten() else { return false };
        if !self.overlays.iter().any(|o| self.copies.get(&o.slot).is_some_and(|c| c.inst.name == unsaved::MOD)) {
            self.open_overlay(mods::new_overlay_id(), spec, window, cx);
        }
        true
    }
}

/// Whether an update is downloaded, so that `app.update` restarts into it.
fn update_ready() -> bool {
    #[cfg(test)]
    if UPDATE_READY.get() {
        return true;
    }
    crate::update::ready().is_some()
}

// Only the update checker's own thread marks one ready: a test says so here.
#[cfg(test)]
thread_local! {
    pub(super) static UPDATE_READY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
