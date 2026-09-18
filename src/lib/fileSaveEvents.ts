export const FILE_SAVED_EVENT = "yuzora:file-saved"
export function notifyFileSaved(path: string): void {
    window.dispatchEvent(new CustomEvent<string>(FILE_SAVED_EVENT, { detail: path }))
}
