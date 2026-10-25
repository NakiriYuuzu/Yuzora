/** "5 minutes ago" in `locale`, "now" under a minute; null when `at` is not a date. */
export function companionAgo(at: string, now: number, locale: string): string | null {
  const then = Date.parse(at)
  if (Number.isNaN(then)) return null
  // A time a little ahead of `now` is negative, so it reads as just now too.
  const seconds = Math.floor((now - then) / 1000)
  const format = new Intl.RelativeTimeFormat(locale, { numeric: "auto" })
  if (seconds < 60) return format.format(0, "second")
  if (seconds < 3600) return format.format(-Math.floor(seconds / 60), "minute")
  if (seconds < 86_400) return format.format(-Math.floor(seconds / 3600), "hour")
  return format.format(-Math.floor(seconds / 86_400), "day")
}
