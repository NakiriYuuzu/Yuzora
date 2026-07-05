import { useEffect } from "react"
import { listen } from "@tauri-apps/api/event"

import { useWorkspaceStore } from "../state/workspaceStore"
import { useGitStore } from "../state/gitStore"

export function GitBridge() {
    const workspacePath = useWorkspaceStore((s) => s.workspacePath)
    const remoteCheck = useGitStore((s) => s.remoteCheck)

    // effect A: detect git environment whenever the workspace changes.
    useEffect(() => {
        if (!workspacePath) return
        void useGitStore.getState().detect(workspacePath)
    }, [workspacePath])

    // effect B: refresh on backend/fs events; on window focus also poll remote.
    useEffect(() => {
        const onFocus = () => {
            void useGitStore.getState().refresh()
            void useGitStore.getState().checkRemote()
        }
        const unlistenState = listen("git:state-changed", () => {
            // .git 變動可能改到 refs/HEAD（branch/checkout/commit）：同時重載 branches，
            // 否則 ahead/behind 與 current branch 會滯後。
            void useGitStore.getState().refresh()
            void useGitStore.getState().loadBranches()
        })
        const unlistenFs = listen("fs:external-change", () => {
            void useGitStore.getState().refresh()
        })
        window.addEventListener("focus", onFocus)
        return () => {
            void unlistenState.then((fn) => fn())
            void unlistenFs.then((fn) => fn())
            window.removeEventListener("focus", onFocus)
        }
    }, [])

    // effect C: periodic remote check, rebuilt when config changes. Skip a round
    // while the window is unfocused so background polling stays quiet.
    useEffect(() => {
        if (remoteCheck.mode === "off") return
        const id = setInterval(() => {
            if (!document.hasFocus()) return
            void useGitStore.getState().checkRemote()
        }, remoteCheck.intervalSec * 1000)
        return () => clearInterval(id)
    }, [remoteCheck])

    return null
}
