import type { ConnectionOwner } from "./runtimeIdentity"

export type PreviewResourceSource =
    | { kind: "local"; workspace: string }
    | { kind: "sftp"; sessionId: string; root: string }
    | { kind: "runtime"; owner: ConnectionOwner; workspace: string }

export interface PreviewResourceLease {
    id: string
    url: string
}

export interface PreviewShortcutBinding {
    id: string
    key: string
    ctrl: boolean
    meta: boolean
    alt: boolean
    shift: boolean
}

/** Selection data comes from the page and must be validated before copying. */
export interface PreviewInteractionSnapshot {
    commands: string[]
    selection: unknown
    selecting: boolean
}
