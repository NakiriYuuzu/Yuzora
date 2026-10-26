/**
 * True when Select All would only highlight the page: focus is on a non-text
 * control (e.g. a checkbox or button) inside an element marked
 * `data-contain-select-all`, such as a dialog over the workbench.
 */
export function keepsSelectAllFromPage(focused: EventTarget | null): boolean {
    return focused instanceof HTMLElement
        && focused.closest("[data-contain-select-all]") !== null
        && !focused.matches("input:not([type=checkbox]), textarea, [contenteditable]")
}
