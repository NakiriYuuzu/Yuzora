// Short relative timestamps for the Log tab commit list (§2 L808 mono 10.5px).
// git log timestamps are unix seconds; we render a compact "5m / 3h / 2d / 3w"
// form for anything under ~30 days and fall back to a short "Mon D" date beyond
// that (matching the design prototype's mixed relative/absolute date column).

const MONTHS = [
    "Jan",
    "Feb",
    "Mar",
    "Apr",
    "May",
    "Jun",
    "Jul",
    "Aug",
    "Sep",
    "Oct",
    "Nov",
    "Dec"
]

const MINUTE = 60
const HOUR = 60 * MINUTE
const DAY = 24 * HOUR
const WEEK = 7 * DAY

// `timestamp` is unix seconds (git log author time). `now` is injectable for
// deterministic tests; defaults to the current wall clock.
export function relativeTime(timestamp: number, now: Date = new Date()): string {
    const nowSec = Math.floor(now.getTime() / 1000)
    const diff = nowSec - timestamp

    // Future / just-now timestamps read as "now" rather than a negative age.
    if (diff < MINUTE) return "now"
    if (diff < HOUR) return `${Math.floor(diff / MINUTE)}m`
    if (diff < DAY) return `${Math.floor(diff / HOUR)}h`
    // Under 30 days: days then weeks. 30d+ falls through to a short date so the
    // column never shows an unwieldy "9w".
    if (diff < 30 * DAY) {
        if (diff < WEEK) return `${Math.floor(diff / DAY)}d`
        return `${Math.floor(diff / WEEK)}w`
    }
    const d = new Date(timestamp * 1000)
    return `${MONTHS[d.getMonth()]} ${d.getDate()}`
}

// Full committed-date string for the details panel (§2 L834 mono 10px, e.g.
// "2026-07-03 14:22"). Local time, zero-padded.
export function fullDateTime(timestamp: number): string {
    const d = new Date(timestamp * 1000)
    const yyyy = d.getFullYear()
    const mm = String(d.getMonth() + 1).padStart(2, "0")
    const dd = String(d.getDate()).padStart(2, "0")
    const hh = String(d.getHours()).padStart(2, "0")
    const min = String(d.getMinutes()).padStart(2, "0")
    return `${yyyy}-${mm}-${dd} ${hh}:${min}`
}
