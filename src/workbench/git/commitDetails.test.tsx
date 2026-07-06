import { render, screen, fireEvent } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import type { CommitDetail, LogCommit } from "@/lib/types"
import { CommitDetails } from "./CommitDetails"

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
    writeText: vi.fn(async () => undefined)
}))

const commit: LogCommit = {
    hash: "abcdef1234567890abcdef1234567890abcdef12",
    shortHash: "abcdef1",
    subject: "feat: add cherry-pick",
    authorName: "Sora",
    authorEmail: "sora@yuuzu.dev",
    timestamp: 1_770_000_000,
    parents: ["1234567890abcdef1234567890abcdef12345678"],
    refs: []
}

const detail: CommitDetail = {
    subject: commit.subject,
    body: "",
    authorName: commit.authorName,
    authorEmail: commit.authorEmail,
    timestamp: commit.timestamp,
    parents: commit.parents,
    files: [
        { status: "M", path: "src/git.rs", oldPath: null, additions: 3, deletions: 1, binary: false }
    ],
    totalAdditions: 3,
    totalDeletions: 1
}

describe("CommitDetails Cherry-pick action", () => {
    it("enables Cherry-pick and calls onCherryPick with the selected hash", () => {
        const onCherryPick = vi.fn()
        render(
            <CommitDetails
                selectedCommit={commit}
                detail={detail}
                detailLoading={false}
                onCheckout={vi.fn()}
                onCherryPick={onCherryPick}
            />
        )

        const cherryPick = screen.getByRole("button", { name: "Cherry-pick" })
        expect(cherryPick).not.toBeDisabled()
        fireEvent.click(cherryPick)
        expect(onCherryPick).toHaveBeenCalledWith(commit.hash)
    })

    it("disables Cherry-pick when cherryPickDisabled is true", () => {
        render(
            <CommitDetails
                selectedCommit={commit}
                detail={detail}
                detailLoading={false}
                onCheckout={vi.fn()}
                onCherryPick={vi.fn()}
                cherryPickDisabled
            />
        )

        expect(screen.getByRole("button", { name: "Cherry-pick" })).toBeDisabled()
    })
})
