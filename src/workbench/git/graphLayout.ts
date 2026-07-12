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
// Closed lane slots are recycled so the lane list stays tight; histories with
// more concurrent lines than MAX_LANES keep full bookkeeping and only clamp at
// the output (see MAX_LANES).

export interface GraphSegment {
    // Lane at the row's top boundary (incoming) and bottom boundary (outgoing).
    // Equal = straight vertical line; different = curve (branch-out / merge-in).
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

// Visual lane cap. Lane BOOKKEEPING is unbounded — capping the tracking itself
// (the old behaviour: overwrite the last slot) destroyed the overwritten lane's
// waiting-parent, so a line still on screen died mid-row and its parent later
// re-materialised as a false new tip. With --all feeding the graph, >12
// concurrent branch lines is an everyday shape, not a pathological one. Output
// lane indices clamp to MAX_LANES-1 instead: overflow lanes collapse onto the
// last (offscreen) column while every visible lane keeps correct topology.
export const MAX_LANES = 12

// A slot in the active-lanes array. `parent` is the commit hash this lane is
// waiting to reach; null marks a free (recyclable) slot.
interface Lane {
    parent: string | null
    colorIdx: number
}

export function computeGraphLayout(
    commits: GraphInputCommit[],
    maxLanes = MAX_LANES
): GraphLayout {
    const rows: GraphRow[] = []
    // Active lanes carried between rows. Index = lane number.
    const lanes: Lane[] = []
    let nextColorIdx = 0
    let widest = 0

    // Find the leftmost free slot (recycled) or append a new one. Never caps:
    // the cap is applied to OUTPUT indices only (clampLane), so tracking stays
    // correct for every in-flight line. Returns the chosen lane index.
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

    // Overflow lanes collapse onto the last visual lane in the output.
    const clampLane = (lane: number): number => Math.min(lane, maxLanes - 1)

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

        // Build segments from the TRUE lane states, then clamp for output.
        const segments = buildSegments(incoming, lanes, commit.hash, nodeLane).map((s) => ({
            ...s,
            fromLane: clampLane(s.fromLane),
            toLane: clampLane(s.toLane)
        }))

        rows.push({ hash: commit.hash, lane: clampLane(nodeLane), colorIdx, isMerge, segments })
        // Widen for the incoming/outgoing boundaries and the node lane itself (a
        // lone root commit closes its only lane, but the column still needs it).
        widest = Math.max(widest, incoming.length, lanes.length, nodeLane + 1)
    }

    return { rows, laneCount: Math.min(widest, maxLanes) }
}

// Derive the line segments for one row from the top boundary (`incoming`) and
// bottom boundary (`outgoing`) lane states. A segment spans the full row height:
// `fromLane` is a top-edge lane, `toLane` a bottom-edge lane. Straight lines pass
// through the node (or a bystander lane); branch-outs and merge-ins bend.
function buildSegments(
    incoming: Lane[],
    outgoing: Lane[],
    commitHash: string,
    nodeLane: number
): GraphSegment[] {
    const segments: GraphSegment[] = []
    // Mark outgoing lanes already reached by a pass-through line so the node only
    // fans out to genuinely new lanes (merge parents / a fresh tip's first parent).
    const claimed = new Array<boolean>(outgoing.length).fill(false)

    // 1. Continuing / merging lines: every in-flight incoming lane routes to its
    //    bottom position — the node lane if it reaches this commit, else the
    //    outgoing lane still carrying the same parent hash.
    for (let i = 0; i < incoming.length; i++) {
        const inc = incoming[i]
        if (inc.parent === null) continue
        let to: number
        if (inc.parent === commitHash) {
            // Arrives at the node (straight if it was the node lane, else a
            // merge-in curve); continues below on the node's lane.
            to = nodeLane
        } else {
            const found = findOutgoingLane(outgoing, inc.parent, inc.colorIdx, i)
            if (found === -1) continue
            to = found
        }
        segments.push({ fromLane: i, toLane: to, colorIdx: inc.colorIdx })
        if (to < claimed.length) claimed[to] = true
    }

    // 2. Node-originated lines: any outgoing lane not claimed by a pass-through
    //    starts at the node — a fresh tip's first-parent line and every extra
    //    merge-parent lane the node just opened.
    for (let j = 0; j < outgoing.length; j++) {
        if (outgoing[j].parent === null || claimed[j]) continue
        segments.push({ fromLane: nodeLane, toLane: j, colorIdx: outgoing[j].colorIdx })
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
