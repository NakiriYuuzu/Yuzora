import { RangeSetBuilder, StateField, type Extension, type Text } from "@codemirror/state"
import {
    Decoration,
    type DecorationSet,
    EditorView,
    WidgetType
} from "@codemirror/view"

export interface ConflictBlock {
    from: number
    to: number
    oursFrom: number
    oursTo: number
    theirsFrom: number
    theirsTo: number
}

type Choice = "current" | "incoming" | "both"

// Scan `docText` for git conflict blocks. A block starts on a line beginning
// with "<<<<<<<", is split by a "=======" line, and ends on a ">>>>>>>" line.
// The zdiff3 base section ("|||||||" … until "=======") is folded into ours'
// end but its content is ignored. Blocks missing their end marker are dangling
// and dropped (unpaired → no action). Offsets are absolute string indices;
// ours/theirs ranges cover the content lines *including* their trailing "\n"
// (or EOF), so slicing them yields exactly the text to keep on resolution.
export function scanConflicts(docText: string): ConflictBlock[] {
    const blocks: ConflictBlock[] = []
    const len = docText.length
    let pos = 0
    while (pos <= len) {
        const nl = docText.indexOf("\n", pos)
        const nextLineStart = nl === -1 ? len + 1 : nl + 1
        const line = docText.slice(pos, nl === -1 ? len : nl)

        if (line.startsWith("<<<<<<<")) {
            const parsed = parseBlock(docText, pos, nextLineStart, len)
            if (parsed) {
                blocks.push(parsed.block)
                pos = parsed.next
                continue
            }
            // Dangling start with no matching end marker → skip past this line.
        }
        pos = nextLineStart
    }
    return blocks
}

// Parse one block whose start marker occupies [startPos, oursStart). Returns the
// block plus the scan resume offset, or null if the block never closes.
function parseBlock(
    docText: string,
    startPos: number,
    oursStart: number,
    len: number
): { block: ConflictBlock; next: number } | null {
    let pos = oursStart
    const oursFrom = oursStart
    let oursTo = -1 // set at "|||||||" or "=======", whichever comes first
    let theirsFrom = -1
    let inTheirs = false

    while (pos <= len) {
        const nl = docText.indexOf("\n", pos)
        const nextLineStart = nl === -1 ? len + 1 : nl + 1
        const line = docText.slice(pos, nl === -1 ? len : nl)

        if (!inTheirs && line.startsWith("|||||||")) {
            if (oursTo === -1) oursTo = pos
        } else if (!inTheirs && line.startsWith("=======")) {
            if (oursTo === -1) oursTo = pos
            theirsFrom = nextLineStart
            inTheirs = true
        } else if (inTheirs && line.startsWith(">>>>>>>")) {
            return {
                block: {
                    from: startPos,
                    to: Math.min(nextLineStart, len),
                    oursFrom,
                    oursTo,
                    theirsFrom,
                    theirsTo: pos
                },
                next: nextLineStart
            }
        } else if (line.startsWith("<<<<<<<")) {
            // A new start before this one closed → the current block is dangling.
            return null
        }
        pos = nextLineStart
    }
    // Reached EOF without a ">>>>>>>" marker → dangling.
    return null
}

// Resolve the block at `index` per `choice`, replacing the whole block span with
// the kept text and removing the three marker lines. Exported so both the widget
// and tests drive resolution through one path (plan Interfaces).
export function applyResolution(view: EditorView, index: number, choice: Choice) {
    const text = view.state.doc.toString()
    const block = scanConflicts(text)[index]
    if (!block) return

    const ours = text.slice(block.oursFrom, block.oursTo)
    const theirs = text.slice(block.theirsFrom, block.theirsTo)
    const kept = choice === "current" ? ours : choice === "incoming" ? theirs : ours + theirs

    view.dispatch({ changes: { from: block.from, to: block.to, insert: kept } })
}

// The three-button widget rendered at the top of each conflict block. It holds
// only the block index; the EditorView is resolved from the DOM at click time
// (EditorView.findFromDOM) so the widget can live in a StateField, which CM6
// requires for block-level widgets.
class ResolveWidget extends WidgetType {
    constructor(readonly index: number) {
        super()
    }

    eq(other: ResolveWidget) {
        return other.index === this.index
    }

    toDOM() {
        const wrap = document.createElement("div")
        wrap.className = "cm-conflict-actions"
        wrap.setAttribute("aria-hidden", "true")
        const mk = (label: string, choice: Choice) => {
            const btn = document.createElement("button")
            btn.type = "button"
            btn.textContent = label
            btn.className = "cm-conflict-btn"
            btn.onmousedown = (e) => e.preventDefault()
            btn.onclick = () => {
                const view = EditorView.findFromDOM(wrap)
                if (view) applyResolution(view, this.index, choice)
            }
            wrap.appendChild(btn)
        }
        mk("Accept current", "current")
        mk("Accept incoming", "incoming")
        mk("Accept both", "both")
        return wrap
    }

    ignoreEvent() {
        return false
    }
}

// Build ours/theirs/marker line decorations plus the action widget for every
// conflict block in the current document.
function buildDecorations(doc: Text): DecorationSet {
    const builder = new RangeSetBuilder<Decoration>()
    const blocks = scanConflicts(doc.toString())
    const oursDeco = Decoration.line({ class: "cm-conflict-ours" })
    const theirsDeco = Decoration.line({ class: "cm-conflict-theirs" })
    const markerDeco = Decoration.line({ class: "cm-conflict-marker" })

    for (let i = 0; i < blocks.length; i++) {
        const block = blocks[i]
        const startLine = doc.lineAt(block.from)
        builder.add(
            startLine.from,
            startLine.from,
            Decoration.widget({ widget: new ResolveWidget(i), side: -1, block: true })
        )
        let pos = block.from
        while (pos < block.to) {
            const line = doc.lineAt(pos)
            let deco = markerDeco
            if (line.from >= block.oursFrom && line.from < block.oursTo) deco = oursDeco
            else if (line.from >= block.theirsFrom && line.from < block.theirsTo) deco = theirsDeco
            builder.add(line.from, line.from, deco)
            pos = line.to + 1
        }
    }
    return builder.finish()
}

// A StateField (required for the block-level action widget) holding the current
// decoration set; recomputed whenever the document changes.
const conflictField = StateField.define<DecorationSet>({
    create(state) {
        return buildDecorations(state.doc)
    },
    update(deco, tr) {
        return tr.docChanged ? buildDecorations(tr.newDoc) : deco
    },
    provide: (f) => EditorView.decorations.from(f)
})

export function conflictMarkers(): Extension {
    return conflictField
}
