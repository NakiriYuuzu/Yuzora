import { writeText } from "@tauri-apps/plugin-clipboard-manager"
import { useTranslation } from "react-i18next"

import type { CommitDetail, CommitFileChange, LogCommit } from "@/lib/types"
import { fullDateTime } from "@/lib/relativeTime"
import { authorColor, authorInitials } from "@/workbench/git/logColors"

// §5 gitBadge palette (design L3206-3210) — reused for the changed-file rows.
const BADGE_COLORS: Record<string, { fg: string; bg: string }> = {
    M: { fg: "#2456cc", bg: "var(--blue-soft)" },
    A: { fg: "#178a63", bg: "var(--mint-soft)" },
    D: { fg: "#c2293f", bg: "var(--danger-soft)" },
    R: { fg: "#9a6512", bg: "var(--amber-soft)" },
    U: { fg: "#6b6760", bg: "var(--paper-3)" }
}

function badgeChar(status: string): string {
    const c = status.charAt(0).toUpperCase()
    return c in BADGE_COLORS ? c : "M"
}

function FileBadge({ badge }: { badge: string }) {
    const { fg, bg } = BADGE_COLORS[badge] ?? BADGE_COLORS.M
    return (
        <span
            aria-hidden="true"
            className="flex size-[18px] shrink-0 items-center justify-center rounded-[6px] font-mono text-[10px] font-bold"
            style={{ background: bg, color: fg }}
        >
            {badge}
        </span>
    )
}

function splitPath(path: string): { name: string; dir: string } {
    const idx = path.lastIndexOf("/")
    if (idx < 0) return { name: path, dir: "" }
    return { name: path.slice(idx + 1), dir: path.slice(0, idx + 1) }
}

// §2 L848-855 changed-file row. Click behaviour (Diff modal) is wired by T6;
// here the row is a button with no side effect so the surface exists but is a
// no-op — see FileRow's onClick.
function FileRow({ file, onOpen }: { file: CommitFileChange; onOpen?: (file: CommitFileChange) => void }) {
    const { name, dir } = splitPath(file.path)
    const badge = badgeChar(file.status)
    return (
        <button
            type="button"
            onClick={onOpen ? () => onOpen(file) : undefined}
            className="flex h-[30px] w-full items-center gap-[9px] rounded-[8px] px-[8px] my-[1px] text-left transition-[background] duration-[120ms] hover:bg-(--yz-panel)"
        >
            <FileBadge badge={badge} />
            <span className="min-w-0 flex-1 truncate">
                <span className="text-[12px] font-medium text-(--ink-1)">{name}</span>
                {dir && <span className="ml-[6px] text-[10px] text-(--ink-4)">{dir}</span>}
            </span>
            {file.additions > 0 && (
                <span className="shrink-0 font-mono text-[10px]" style={{ color: "#178a63" }}>
                    +{file.additions}
                </span>
            )}
            {file.deletions > 0 && (
                <span className="shrink-0 font-mono text-[10px]" style={{ color: "#c2293f" }}>
                    −{file.deletions}
                </span>
            )}
        </button>
    )
}

function FooterButton({
    label,
    disabled,
    danger,
    title,
    onClick,
    children
}: {
    label: string
    disabled?: boolean
    danger?: boolean
    title?: string
    onClick?: () => void
    children?: React.ReactNode
}) {
    // §2 L857-864 footer buttons — h30, r9. Enabled: solid track + border.
    // The danger "Reset" is a disabled red-outline button in the design itself.
    return (
        <button
            type="button"
            aria-label={label}
            title={title}
            disabled={disabled || danger}
            onClick={onClick}
            className={
                "flex h-[30px] items-center gap-[5px] rounded-[9px] px-[12px] text-[11.5px] font-semibold transition-transform active:scale-[0.97] " +
                (danger
                    ? "cursor-not-allowed opacity-[0.85]"
                    : disabled
                      ? "cursor-not-allowed border border-(--line-1) bg-(--yz-solid) text-(--ink-1) opacity-50"
                      : "cursor-pointer border border-(--line-1) bg-(--yz-solid) text-(--ink-1) shadow-(--shadow-xs)")
            }
            style={
                danger
                    ? { border: "1px solid rgba(226,59,84,0.36)", color: "#c2293f" }
                    : undefined
            }
        >
            {children}
            {label}
        </button>
    )
}

/**
 * §2 L817-866 commit details right column (240px). Header (hash chip + copy,
 * subject, author avatar, committed/parents), changed-files list, and the
 * footer action row. Checkout / Cherry-pick go through injected callbacks;
 * Compare stays guarded by loaded details. File-row clicks are surfaced via
 * `onOpenFile` for T6 to wire the Diff modal.
 */
export function CommitDetails({
    selectedCommit,
    detail,
    detailLoading,
    onCheckout,
    onOpenFile,
    onCompare,
    onCherryPick,
    cherryPickDisabled
}: {
    selectedCommit: LogCommit | null
    detail: CommitDetail | null
    detailLoading: boolean
    onCheckout: (hash: string) => void
    onOpenFile?: (file: CommitFileChange) => void
    onCompare?: (hash: string) => void
    onCherryPick?: (hash: string) => void
    cherryPickDisabled?: boolean
}) {
    const { t } = useTranslation("menus")
    if (!selectedCommit) {
        return (
            <div className="flex w-[240px] shrink-0 flex-col border-l border-(--line-1) bg-(--paper-1)">
                <div className="flex flex-1 items-center justify-center px-[16px] text-center text-[12.5px] text-(--ink-3)">
                    {t("commitDetails.selectPrompt")}
                </div>
            </div>
        )
    }

    return (
        <div className="flex w-[240px] shrink-0 flex-col border-l border-(--line-1) bg-(--paper-1)">
            {/* header */}
            <div className="border-b border-(--line-1) px-[16px] pb-[13px] pt-[14px]">
                <div className="mb-[10px] flex items-center gap-[8px]">
                    <span
                        className="rounded-[6px] px-[8px] py-[3px] font-mono text-[11px] font-semibold"
                        style={{ color: "#3b6fe0", background: "var(--blue-soft)" }}
                    >
                        {selectedCommit.shortHash}
                    </span>
                    <div className="flex-1" />
                    <button
                        type="button"
                        aria-label={t("commitDetails.copyHashAriaLabel")}
                        title={t("commitDetails.copyHashAriaLabel")}
                        onClick={() => void writeText(selectedCommit.hash)}
                        className="flex size-[26px] items-center justify-center rounded-[7px] text-(--ink-3) transition-all duration-150 hover:bg-(--paper-2) hover:text-(--ink-1)"
                    >
                        <svg
                            width="13"
                            height="13"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            strokeWidth="1.8"
                            strokeLinecap="round"
                            strokeLinejoin="round"
                            aria-hidden="true"
                        >
                            <rect x="9" y="9" width="11" height="11" rx="2" />
                            <path d="M5 15V5a2 2 0 0 1 2-2h10" />
                        </svg>
                    </button>
                </div>

                <div className="font-serif text-[15.5px] font-medium leading-[1.35] text-(--ink-0)">
                    {selectedCommit.subject}
                </div>

                <div className="mt-[13px] flex items-center gap-[10px]">
                    <span
                        className="flex size-[30px] shrink-0 items-center justify-center rounded-full text-[12px] font-bold text-white shadow-(--shadow-xs)"
                        style={{ background: authorColor(selectedCommit.authorName) }}
                        aria-hidden="true"
                    >
                        {authorInitials(selectedCommit.authorName)}
                    </span>
                    <div className="flex min-w-0 flex-col gap-[1px]">
                        <span className="text-[12px] font-semibold text-(--ink-1)">
                            {selectedCommit.authorName}
                        </span>
                        <span className="truncate font-mono text-[10px] text-(--ink-3)">
                            {selectedCommit.authorEmail}
                        </span>
                    </div>
                </div>

                <div className="mt-[12px] flex items-center gap-[8px] font-mono text-[10px] text-(--ink-3)">
                    <span className="text-(--ink-4)">{t("commitDetails.committedLabel")}</span>
                    {fullDateTime(selectedCommit.timestamp)}
                </div>
                <div className="mt-[4px] flex items-center gap-[8px] font-mono text-[10px] text-(--ink-3)">
                    <span className="text-(--ink-4)">{t("commitDetails.parentsLabel")}</span>
                    {selectedCommit.parents.length > 0
                        ? selectedCommit.parents.map((p) => p.slice(0, 7)).join(" · ")
                        : "—"}
                </div>
            </div>

            {detailLoading && !detail ? (
                <div className="flex flex-1 items-center justify-center text-[11.5px] text-(--ink-3)">
                    {t("commitDetails.loadingEllipsis")}
                </div>
            ) : (
                <>
                    {/* changed files header */}
                    <div className="flex items-center gap-[8px] px-[16px] pb-[6px] pt-[11px]">
                        <span className="font-sans text-[9.5px] font-bold uppercase tracking-[0.07em] text-(--ink-3)">
                            {t("commitDetails.changedFilesLabel")}
                        </span>
                        <span className="font-mono text-[10px] text-(--ink-4)">
                            {detail ? t("commitDetails.fileCount", { count: detail.files.length }) : ""}
                        </span>
                        <div className="flex-1" />
                        {detail && (
                            <>
                                <span className="font-mono text-[10.5px]" style={{ color: "#178a63" }}>
                                    +{detail.totalAdditions}
                                </span>
                                <span className="font-mono text-[10.5px]" style={{ color: "#c2293f" }}>
                                    −{detail.totalDeletions}
                                </span>
                            </>
                        )}
                    </div>

                    {/* changed files list */}
                    <div className="yzs min-h-0 flex-1 overflow-auto px-[10px]">
                        {detail?.files.map((file) => (
                            <FileRow key={file.path} file={file} onOpen={onOpenFile} />
                        ))}
                    </div>
                </>
            )}

            {/* footer actions */}
            <div className="flex flex-wrap gap-[7px] border-t border-(--line-1) px-[14px] py-[11px]">
                <FooterButton
                    label={t("commitDetails.checkout")}
                    onClick={() => onCheckout(selectedCommit.hash)}
                />
                <FooterButton
                    label={t("commitDetails.compare")}
                    disabled={!onCompare || !detail}
                    title={t("commitDetails.openInDiffViewerTitle")}
                    onClick={
                        onCompare && detail
                            ? () => onCompare(selectedCommit.hash)
                            : undefined
                    }
                />
                <FooterButton
                    label={t("commitDetails.cherryPick")}
                    disabled={!onCherryPick || cherryPickDisabled}
                    title={t("commitDetails.cherryPickThisCommitTitle")}
                    onClick={onCherryPick ? () => onCherryPick(selectedCommit.hash) : undefined}
                />
                <FooterButton
                    label={t("commitDetails.resetMainToHere")}
                    danger
                    title={t("commitDetails.resetHoldTitle")}
                >
                    <svg
                        width="12"
                        height="12"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        strokeWidth="2"
                        strokeLinecap="round"
                        strokeLinejoin="round"
                        aria-hidden="true"
                    >
                        <rect x="4" y="11" width="16" height="9" rx="2" />
                        <path d="M8 11V7a4 4 0 0 1 8 0v4" />
                    </svg>
                </FooterButton>
            </div>
        </div>
    )
}
