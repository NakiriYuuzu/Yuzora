// Commit-graph lane layout — pure function, no React/store imports. Turns a
// timestamp-descending commit list (git log order) into per-row lane assignments
// and line segments that a JetBrains-style graph renderer can draw directly.
//
// Standard "active lanes" algorithm: we keep a list of in-progress lanes, each
// waiting for a specific parent hash. Walking rows top-to-bottom (newest first):
//   - the commit occupies the leftmost lane waiting for its hash (multiple lanes
//     waiting for the same hash = a merge point; they converge to one, the rest
//     close);
//   - a commit nobody waits for is a new branch tip and opens a fresh lane with a
//     new colour;
//   - after the node, its lane waits for the first parent; remaining parents each
//     open (or reuse an existing lane already waiting for the same hash).
// Closed lane slots are recycled so the lane list stays tight. Every concurrent
// line retains its own lane in the output; the renderer sizes to laneCount.

export interface GraphSegment {
    // Through spans both boundaries; incoming ends at the middle node and
    // outgoing starts there, so every edge meets its actual commit.
    phase: "through" | "incoming" | "outgoing"
    fromLane: number
    toLane: number
    colorIdx: number
}

export interface GraphRow {
    hash: string
    lane: number // this commit's node lane (0-based, 0 = leftmost)
    colorIdx: number // node/lane colour index (renderer takes % palette length)
    isMerge: boolean // parents.length > 1
    segments: GraphSegment[] // lines to draw across this row
}

export interface GraphLayout {
    rows: GraphRow[]
    laneCount: number // widest row's lane count (for sizing the SVG column)
}

export interface GraphInputCommit {
    hash: string
    parents: string[]
}

// A slot in the active-lanes array. `parent` is the commit hash this lane is
// waiting to reach; null marks a free (recyclable) slot.
interface Lane {
    parent: string | null
    colorIdx: number
}

export function computeGraphLayout(commits: GraphInputCommit[]): GraphLayout {
    const rows: GraphRow[] = []
    // Active lanes carried between rows. Index = lane number.
    const lanes: Lane[] = []
    let nextColorIdx = 0
    let widest = 0

    // Find the leftmost free slot (recycled) or append a new one.
    const allocLane = (parent: string, colorIdx: number): number => {
        for (let i = 0; i < lanes.length; i++) {
            if (lanes[i].parent === null) {
                lanes[i] = { parent, colorIdx }
                return i
            }
        }
        lanes.push({ parent, colorIdx })
        return lanes.length - 1
    }

    for (const commit of commits) {
        // Snapshot the incoming lane boundary (top of this row) so segments can
        // reference where each line entered the row.
        const incoming: Lane[] = lanes.map((l) => ({ ...l }))

        // Lanes waiting for THIS commit. Leftmost becomes the node lane; the rest
        // are merge tails that converge here and close.
        const waiting: number[] = []
        for (let i = 0; i < lanes.length; i++) {
            if (lanes[i].parent === commit.hash) waiting.push(i)
        }

        let nodeLane: number
        let colorIdx: number
        if (waiting.length > 0) {
            nodeLane = waiting[0]
            colorIdx = lanes[nodeLane].colorIdx
            // Close the extra lanes that were also waiting for this hash (merge).
            for (let k = 1; k < waiting.length; k++) {
                lanes[waiting[k]] = { parent: null, colorIdx: lanes[waiting[k]].colorIdx }
            }
        } else {
            // New branch tip: nobody was waiting for this commit.
            colorIdx = nextColorIdx++
            nodeLane = allocLane(commit.hash, colorIdx)
        }

        const isMerge = commit.parents.length > 1
        const firstParent = commit.parents[0] ?? null

        // The node lane now continues toward the first parent (or closes if this
        // commit is a root with no parents).
        if (firstParent === null) {
            lanes[nodeLane] = { parent: null, colorIdx }
        } else {
            lanes[nodeLane] = { parent: firstParent, colorIdx }
        }

        // Remaining parents (merge sources): reuse a lane already waiting for the
        // same hash, else open a new lane with a new colour.
        for (let p = 1; p < commit.parents.length; p++) {
            const ph = commit.parents[p]
            const existing = lanes.findIndex((l) => l.parent === ph)
            if (existing === -1) allocLane(ph, nextColorIdx++)
        }

        // Trim trailing free lanes so the array (and lane count) stays tight.
        while (lanes.length > 0 && lanes[lanes.length - 1].parent === null) {
            lanes.pop()
        }

        const segments = buildSegments(incoming, lanes, commit.hash, nodeLane, commit.parents)

        rows.push({ hash: commit.hash, lane: nodeLane, colorIdx, isMerge, segments })
        // Widen for the incoming/outgoing boundaries and the node lane itself (a
        // lone root commit closes its only lane, but the column still needs it).
        widest = Math.max(widest, incoming.length, lanes.length, nodeLane + 1)
    }

    return { rows, laneCount: widest }
}

// Split node edges at the row midpoint. Bystander lines still span the row.
function buildSegments(
    incoming: Lane[],
    outgoing: Lane[],
    commitHash: string,
    nodeLane: number,
    parents: readonly string[]
): GraphSegment[] {
    const segments: GraphSegment[] = []
    for (let i = 0; i < incoming.length; i++) {
        const inc = incoming[i]
        if (inc.parent === null) continue
        if (inc.parent === commitHash) {
            segments.push({ fromLane: i, toLane: nodeLane, colorIdx: inc.colorIdx, phase: "incoming" })
        } else {
            const to = findOutgoingLane(outgoing, inc.parent, inc.colorIdx, i)
            if (to !== -1) segments.push({ fromLane: i, toLane: to, colorIdx: inc.colorIdx, phase: "through" })
        }
    }

    // Every parent edge must leave the node, even when another branch already
    // carries that parent through this row. The first parent owns nodeLane.
    for (let p = 0; p < parents.length; p++) {
        const to = p === 0 ? nodeLane : outgoing.findIndex((lane) => lane.parent === parents[p])
        if (to !== -1) segments.push({ fromLane: nodeLane, toLane: to, colorIdx: outgoing[to].colorIdx, phase: "outgoing" })
    }
    return segments
}

// Locate the outgoing lane carrying `parent`. Prefer the same index (a straight
// continuation), otherwise the first lane matching hash+colour, then any lane
// with the hash (colour may have shifted after a merge convergence).
function findOutgoingLane(
    outgoing: Lane[],
    parent: string,
    colorIdx: number,
    preferIdx: number
): number {
    if (
        preferIdx < outgoing.length &&
        outgoing[preferIdx].parent === parent &&
        outgoing[preferIdx].colorIdx === colorIdx
    ) {
        return preferIdx
    }
    for (let i = 0; i < outgoing.length; i++) {
        if (outgoing[i].parent === parent && outgoing[i].colorIdx === colorIdx) return i
    }
    // Colour may differ after a merge convergence; fall back to hash match.
    for (let i = 0; i < outgoing.length; i++) {
        if (outgoing[i].parent === parent) return i
    }
    return -1
}
