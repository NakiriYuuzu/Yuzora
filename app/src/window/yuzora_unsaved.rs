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
        let Some(spec) = self.folders.contains_key(unsaved::MOD).then(|| unsaved::dialog(name, args, &self.model, cx)).flatten() else { return false };
        if !self.overlays.iter().any(|o| self.copies.get(&o.slot).is_some_and(|c| c.inst.name == unsaved::MOD)) {
            self.open_overlay(mods::new_overlay_id(), spec, window, cx);
        }
        true
    }
}
