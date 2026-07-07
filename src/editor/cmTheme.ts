import { EditorView } from "@codemirror/view"
import { HighlightStyle } from "@codemirror/language"
import { tags as t } from "@lezer/highlight"

// App-wide CodeMirror theme. Every colour is a `var(--…)` reference to an app
// token (see src/editor/editor.css: --cm-* chrome tints + --syn-* syntax palette,
// both with .dark overrides), so switching theme = toggling the <html>.dark class
// with no editor reconfigure. Because we don't use the { dark: true } flag, every
// component that CodeMirror's baseTheme gives a default colour must be overridden
// explicitly here — otherwise a light default leaks through in dark mode.
export const appTheme = EditorView.theme({
    "&": {
        color: "var(--ink-1)",
        backgroundColor: "var(--paper-1)"
    },
    ".cm-content": {
        caretColor: "var(--ink-0)"
    },
    ".cm-cursor, .cm-dropCursor": {
        borderLeftColor: "var(--ink-0)"
    },
    "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection": {
        backgroundColor: "var(--cm-selection)"
    },
    ".cm-gutters": {
        backgroundColor: "var(--paper-1)",
        color: "var(--ink-4)",
        border: "none"
    },
    ".cm-activeLine": {
        backgroundColor: "var(--cm-active-line)"
    },
    ".cm-activeLineGutter": {
        backgroundColor: "var(--cm-active-line-gutter)",
        color: "var(--ink-2)"
    },
    ".cm-matchingBracket": {
        backgroundColor: "var(--cm-bracket-bg)",
        outline: "1px solid var(--cm-bracket-border)"
    },
    ".cm-nonmatchingBracket": {
        backgroundColor: "transparent",
        color: "var(--destructive)"
    },
    ".cm-searchMatch": {
        backgroundColor: "var(--cm-search-match)",
        outline: "1px solid rgba(var(--yz-accent-rgb), 0.35)"
    },
    ".cm-searchMatch.cm-searchMatch-selected": {
        backgroundColor: "var(--cm-search-match-selected)"
    },
    ".cm-selectionMatch": {
        backgroundColor: "var(--cm-search-match)"
    },
    ".cm-tooltip": {
        backgroundColor: "var(--yz-glass-strong)",
        color: "var(--ink-1)",
        border: "1px solid var(--line-2)",
        borderRadius: "var(--r-md)",
        boxShadow: "var(--shadow-md)",
        backdropFilter: "var(--blur-frost)"
    },
    ".cm-tooltip.cm-tooltip-arrow:before": {
        borderTopColor: "var(--line-2)",
        borderBottomColor: "var(--line-2)"
    },
    ".cm-tooltip.cm-tooltip-arrow:after": {
        borderTopColor: "var(--yz-glass-strong)",
        borderBottomColor: "var(--yz-glass-strong)"
    },
    ".cm-tooltip-autocomplete": {
        "& > ul": {
            fontFamily: "var(--font-mono, monospace)"
        },
        "& > ul > li": {
            color: "var(--ink-1)"
        },
        "& > ul > li[aria-selected]": {
            backgroundColor: "var(--cm-completion-selected)",
            color: "var(--ink-0)"
        }
    },
    ".cm-completionIcon": {
        color: "var(--ink-3)"
    },
    ".cm-completionLabel": {
        color: "var(--ink-1)"
    },
    ".cm-completionDetail": {
        color: "var(--ink-3)"
    },
    ".cm-completionMatchedText": {
        color: "var(--yz-accent-ink)",
        textDecoration: "none",
        fontWeight: "600"
    },
    ".cm-panels": {
        backgroundColor: "var(--yz-glass-strong)",
        color: "var(--ink-1)"
    },
    ".cm-panels.cm-panels-top": {
        borderBottom: "1px solid var(--line-2)"
    },
    ".cm-panels.cm-panels-bottom": {
        borderTop: "1px solid var(--line-2)"
    },
    ".cm-panel.cm-search label": {
        color: "var(--ink-2)"
    },
    ".cm-panel.cm-search input": {
        backgroundColor: "var(--yz-field)",
        border: "1px solid var(--line-2)",
        borderRadius: "var(--r-xs)",
        color: "var(--ink-1)"
    },
    ".cm-panel.cm-search button": {
        backgroundColor: "var(--yz-solid)",
        border: "1px solid var(--line-2)",
        borderRadius: "var(--r-xs)",
        color: "var(--ink-1)"
    },
    ".cm-foldPlaceholder": {
        backgroundColor: "var(--yz-sunk)",
        color: "var(--ink-3)",
        border: "1px solid var(--line-1)",
        borderRadius: "var(--r-xs)"
    }
})

// Syntax palette. Values are `var(--syn-*)` strings (StyleModule passes them
// through untouched); the variables live on `.cm-editor` in editor.css and carry
// their own .dark overrides, so the highlight follows the theme too. Colours are
// picked to match the --st-* LSP semantic palette (same meaning → same hue).
export const appHighlightStyle = HighlightStyle.define([
    { tag: t.keyword, color: "var(--syn-keyword)" },
    { tag: t.string, color: "var(--syn-string)" },
    { tag: t.comment, color: "var(--syn-comment)", fontStyle: "italic" },
    { tag: t.number, color: "var(--syn-number)" },
    { tag: t.bool, color: "var(--syn-number)" },
    { tag: t.null, color: "var(--syn-number)" },
    { tag: [t.typeName, t.className], color: "var(--syn-type)" },
    { tag: t.function(t.variableName), color: "var(--syn-func)" },
    { tag: t.definition(t.variableName), color: "var(--syn-var)" },
    { tag: t.variableName, color: "var(--syn-var)" },
    { tag: [t.propertyName, t.attributeName], color: "var(--syn-property)" },
    { tag: t.tagName, color: "var(--syn-tag)" },
    { tag: t.operator, color: "var(--syn-operator)" },
    { tag: t.punctuation, color: "var(--syn-punct)" },
    { tag: t.meta, color: "var(--syn-meta)" },
    { tag: t.heading, color: "var(--syn-heading)", fontWeight: "600" },
    { tag: t.link, color: "var(--syn-link)", textDecoration: "underline" },
    { tag: t.emphasis, fontStyle: "italic" },
    { tag: t.strong, fontWeight: "700" }
])
