import { useTranslation } from "react-i18next"
import { PencilIcon } from "lucide-react"

import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover"

import type { ChangeStats } from "./changeStats"

// composer 上方置中的變更統計列（2026-07-21 使用者回饋）：彙總本 session 所有
// diff 的檔案數與增刪行數；點擊展開 per-file 明細。取代先前逐 diff 立卡的顯示。
export function ChangesSummary({ stats }: { stats: ChangeStats }) {
    const { t } = useTranslation("panels")
    return (
        <div style={{ display: "flex", justifyContent: "center" }}>
            <Popover>
                <PopoverTrigger asChild>
                    <button
                        type="button"
                        data-testid="agent-changes-summary"
                        aria-label={t("agentZonePanel.changesAria")}
                        className="inline-flex h-6 cursor-pointer items-center gap-1.5 rounded-full border border-(--line-2) bg-(--yz-glass) px-2.5 font-mono text-[10.5px] text-(--ink-2) backdrop-blur-md"
                    >
                        <PencilIcon aria-hidden="true" size={10} className="text-(--ink-3)" />
                        <span>{t("agentZonePanel.changesFiles", { count: stats.files.length })}</span>
                        <span style={{ color: "var(--yz-accent-ink)", fontWeight: 600 }}>+{stats.added}</span>
                        <span style={{ color: "var(--destructive)", fontWeight: 600 }}>−{stats.removed}</span>
                    </button>
                </PopoverTrigger>
                <PopoverContent side="top" align="center" className="w-80 max-w-[calc(100vw-2rem)] p-2">
                    <ul style={{ listStyle: "none", margin: 0, padding: 0, display: "flex", flexDirection: "column", gap: 4 }}>
                        {stats.files.map((file) => (
                            <li
                                key={file.path}
                                style={{
                                    display: "flex",
                                    alignItems: "center",
                                    gap: 8,
                                    fontFamily: "var(--font-mono)",
                                    fontSize: 11
                                }}
                            >
                                <span
                                    title={file.path}
                                    style={{
                                        flex: 1,
                                        minWidth: 0,
                                        overflow: "hidden",
                                        textOverflow: "ellipsis",
                                        whiteSpace: "nowrap",
                                        direction: "rtl",
                                        textAlign: "left",
                                        color: "var(--ink-1)"
                                    }}
                                >
                                    {file.path}
                                </span>
                                <span style={{ flex: "0 0 auto", color: "var(--yz-accent-ink)" }}>+{file.added}</span>
                                <span style={{ flex: "0 0 auto", color: "var(--destructive)" }}>−{file.removed}</span>
                            </li>
                        ))}
                    </ul>
                </PopoverContent>
            </Popover>
        </div>
    )
}
