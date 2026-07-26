import { useEffect } from "react"
import { listen } from "@tauri-apps/api/event"

import type { ExternalChangePayload, GitStateChangedPayload } from "../lib/types"
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
        const unlistenState = listen<GitStateChangedPayload>("git:state-changed", (e) => {
            // #57 T3：事件帶 workspaceRoot——切換 gap 內舊 workspace 的 .git
            // watcher 仍可能開火，比對 live workspacePath 後才處理（讀事件當下
            // 的值而非 closure 快照，比照 LspBridge 防串場）。
            if (e.payload.workspaceRoot !== useWorkspaceStore.getState().workspacePath) return
            // .git 變動可能改到 refs/HEAD（branch/checkout/commit）：同時重載 branches，
            // 否則 ahead/behind 與 current branch 會滯後。
            void useGitStore.getState().refresh()
            void useGitStore.getState().loadBranches()
        })
        const unlistenFs = listen<ExternalChangePayload>("fs:external-change", (e) => {
            if (e.payload.workspaceRoot !== useWorkspaceStore.getState().workspacePath) return
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
