import { useGitStore } from "../../state/gitStore"

/**
 * GitPanel → Console tab (design §Console dc.html:918-932). Dark terminal-style
 * log of every git operation: newest-first, one block per runOp completion
 * (wired in gitStore.runOp). Each block shows a status dot + `$ {cmd}` in lime
 * with a right-aligned time, then any output lines dimmed and left-indented.
 * Empty state renders when no op has run yet.
 */
export function ConsoleTab() {
    const consoleLog = useGitStore((s) => s.consoleLog)

    return (
        <div
            className="yzs flex min-h-0 flex-1 flex-col overflow-auto font-mono text-[12px] leading-[19px]"
            style={{
                padding: "13px 16px",
                background: "var(--term-bg)",
                color: "var(--term-fg)"
            }}
        >
            {consoleLog.length === 0 ? (
                <div className="text-(--term-fg2)" style={{ padding: "20px 0" }}>
                    No git commands run yet
                </div>
            ) : (
                consoleLog.map((entry) => (
                    <div key={entry.id}>
                        {/* dc.html:922-927 — dot + `$ cmd` (lime 600) + right-aligned time */}
                        <div className="flex items-center gap-[8px]" style={{ marginTop: "10px" }}>
                            <span
                                aria-hidden="true"
                                className="size-[6px] shrink-0 rounded-full"
                                style={{ background: entry.tone === "err" ? "#e23b54" : "#2bbf8a" }}
                            />
                            <span className="font-semibold text-(--term-lime)">
                                $ {entry.cmd}
                            </span>
                            <div className="flex-1" />
                            <span className="text-(--term-fg2) text-[10px]">{entry.time}</span>
                        </div>
                        {/* dc.html:928 — output lines: dimmed, indented, wrapped */}
                        {entry.out.map((line, i) => (
                            <div
                                key={i}
                                className="text-(--term-fg2)"
                                style={{
                                    paddingLeft: "16px",
                                    whiteSpace: "pre-wrap",
                                    wordBreak: "break-word"
                                }}
                            >
                                {line}
                            </div>
                        ))}
                    </div>
                ))
            )}
        </div>
    )
}
