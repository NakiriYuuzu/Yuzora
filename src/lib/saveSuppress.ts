const saved = new Map<string, number>()

export const recentlySaved = {
    mark(path: string) {
        saved.set(path, Date.now())
    },
    snapshot(): Set<string> {
        const now = Date.now()
        const result = new Set<string>()
        for (const [path, at] of saved) {
            // notify-debouncer-mini's default batch_mode delays events up to 2x the
            // debounce timeout (300ms -> 600ms worst case); 750ms leaves margin above that.
            if (now - at < 750) result.add(path)
            else saved.delete(path)
        }
        return result
    }
}
