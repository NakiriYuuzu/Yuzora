import { useTranslation } from "react-i18next"

import { Button } from "@/components/ui/button"
import { fsDeleteCancel } from "@/lib/ipc"
import { patchDeleteTask, useDeleteTasks } from "@/state/deleteProgressStore"

export function DeleteProgressToast({ id }: { id: string }) {
    const { t } = useTranslation("menus")
    const task = useDeleteTasks((tasks) => tasks[id])
    if (!task) return null
    const { progress } = task
    const percent = progress?.phase === "deleting" && progress.total > 0
        ? Math.min(100, Math.round((progress.removed / progress.total) * 100))
        : null
    const detail = !task.local
        ? t("deleteProgress.remote")
        : progress?.phase === "deleting"
            ? t("deleteProgress.deleting", { removed: progress.removed.toLocaleString(), total: progress.total.toLocaleString() })
            : t("deleteProgress.counting", { count: (progress?.found ?? 0).toLocaleString() })

    return (
        <div className="w-[356px] rounded-[var(--radius)] border border-(--border) bg-(--popover) p-[12px] text-(--popover-foreground) shadow-[var(--shadow-xl)]">
            <div className="flex items-center gap-[8px]">
                <p className="min-w-0 flex-1 truncate text-[12.5px] font-semibold">
                    {t("deleteProgress.title", { name: task.name })}
                </p>
                {task.local && (
                    <Button
                        type="button"
                        size="xs"
                        variant="ghost"
                        disabled={task.cancelling}
                        onClick={() => {
                            patchDeleteTask(id, { cancelling: true })
                            void fsDeleteCancel(id).catch(() => undefined)
                        }}
                    >
                        {task.cancelling ? t("deleteProgress.cancelling") : t("deleteProgress.cancel")}
                    </Button>
                )}
            </div>
            <div
                role="progressbar"
                aria-label={t("deleteProgress.label")}
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={percent ?? undefined}
                className="mt-[8px] h-[5px] overflow-hidden rounded-full bg-(--paper-2)"
            >
                {percent === null ? (
                    <div className="yz-load h-full w-1/4 rounded-full bg-(--yz-accent)" />
                ) : (
                    <div className="h-full rounded-full bg-(--yz-accent) transition-[width]" style={{ width: `${percent}%` }} />
                )}
            </div>
            <p className="mt-[6px] text-[11px] text-(--ink-3)">{detail}</p>
        </div>
    )
}
