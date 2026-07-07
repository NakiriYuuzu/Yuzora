import { invoke } from "@/lib/ipc"

export function logUserAction(
    event: string,
    message: string,
    metadata: Record<string, unknown> = {}
): Promise<void> {
    return invoke("log_event", {
        event: {
            level: "info",
            kind: "user_action",
            source: "ui",
            workspace_path: null,
            event,
            message,
            metadata
        }
    })
        .then(() => undefined)
        .catch(() => undefined)
}
