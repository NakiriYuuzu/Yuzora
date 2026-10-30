import type { GraphRow } from "@yuzora/shared/workbench/git/graphLayout"
import { LANE_COLORS } from "@yuzora/shared/workbench/git/logColors"

// The desktop's LogGraph geometry (src/workbench/git/LogGraph.tsx), one small SVG per row.
const LANE_X0 = 10
const LANE_GAP = 12
// Wider graphs are clipped on the right: the subject needs the room more.
const MAX_LANES = 8

const graphWidth = (laneCount: number) => LANE_X0 * 2 + (Math.min(Math.max(laneCount, 1), MAX_LANES) - 1) * LANE_GAP

const laneX = (lane: number) => LANE_X0 + lane * LANE_GAP

/** One commit's slice of the lane graph: its node, the lines passing through and the edges to its parents. */
export function CommitGraph({ row, laneCount, height, hasRefs }: { row: GraphRow; laneCount: number; height: number; hasRefs: boolean }) {
  const cy = height / 2
  const color = (index: number) => LANE_COLORS[index % LANE_COLORS.length]
  return (
    <svg width={graphWidth(laneCount)} height={height} fill="none" aria-hidden className="shrink-0">
      {row.segments.map((segment, index) => {
        const x1 = laneX(segment.fromLane)
        const x2 = laneX(segment.toLane)
        const top = segment.phase === "outgoing" ? cy : 0
        const bottom = segment.phase === "incoming" ? cy : height
        const bend = (top + bottom) / 2
        const d = segment.fromLane === segment.toLane ? `M${x1} ${top} L${x2} ${bottom}` : `M${x1} ${top} C${x1} ${bend} ${x2} ${bend} ${x2} ${bottom}`
        return <path key={index} d={d} stroke={color(segment.colorIdx)} strokeWidth={2} strokeLinecap="round" />
      })}
      <circle cx={laneX(row.lane)} cy={cy} r={hasRefs ? 5.2 : 4.5} fill={color(row.colorIdx)} stroke="var(--yz-node-ring)" strokeWidth={2} />
      {hasRefs && <circle cx={laneX(row.lane)} cy={cy} r={1.8} fill="var(--yz-node-ring)" />}
    </svg>
  )
}
