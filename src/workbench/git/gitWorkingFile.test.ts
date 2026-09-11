import { beforeEach, describe, expect, it, vi } from "vitest"
import { initialGitState, clearGitSnapshots, useGitStore } from "@/state/gitStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { remoteFilePath } from "@/lib/runtimeIdentity"
import { gitChangeRows } from "./gitChangeSelection"
import { gitWorkingFilePath, openGitWorkingFile } from "./gitWorkingFile"

beforeEach(() => {
    clearGitSnapshots()
    useGitStore.setState(initialGitState)
})

function prepare(root: string, path = "src/a.ts", status = "M") {
    useGitStore.setState({
        environment: { status: "ready", root, version: "2.50" },
        status: { branch: "main", headOid: "a".repeat(40), detached: false, upstream: null, ahead: 0, behind: 0,
            staged: [], unstaged: [{ path, status, origPath: null }], untracked: [], conflicted: [], inProgress: null }
    })
    return gitChangeRows(useGitStore.getState().status)[0]
}

describe("current Git working-file menu target", () => {
    it("preserves local Windows paths and remote workspace identity", () => {
        const row = prepare("C:\\work")
        expect(gitWorkingFilePath("C:\\work", row)).toBe("C:\\work\\src\\a.ts")
        const root = remoteFilePath("host-a", "/repo", "/workspace")
        const remoteRow = prepare(root)
        expect(gitWorkingFilePath(root, remoteRow)).toBe(remoteFilePath("host-a", "/repo/src/a.ts", "/workspace"))
    })
    it("opens the clicked live file via the existing editor action", () => {
        const row = prepare("/repo")
        const open = vi.spyOn(useWorkspaceStore.getState(), "openTab").mockImplementation(() => {})
        expect(openGitWorkingFile("/repo", row)).toBe(true)
        expect(open).toHaveBeenCalledWith("/repo/src/a.ts")
        open.mockRestore()
    })
    it("rejects deleted, historical, stale and changed-root targets", () => {
        const deleted = prepare("/repo", "gone.ts", "D")
        expect(gitWorkingFilePath("/repo", deleted)).toBeNull()
        const row = prepare("/repo")
        expect(gitWorkingFilePath("/repo", { ...row, path: "historical.ts" })).toBeNull()
        expect(gitWorkingFilePath("/other", row)).toBeNull()
        useGitStore.setState({ snapshotStale: true })
        expect(gitWorkingFilePath("/repo", row)).toBeNull()
    })
})
