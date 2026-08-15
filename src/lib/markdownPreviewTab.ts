export const MARKDOWN_PREVIEW_PREFIX = "yuzora://markdown-preview/"
export const MARKDOWN_PREVIEW_TAB_NAME = "Preview"

export interface PreviewTabLike {
    kind?: string
    path: string
    sourcePath?: string
}

export function markdownPreviewPath(sourcePath: string): string {
    return `${MARKDOWN_PREVIEW_PREFIX}${encodeURIComponent(sourcePath)}`
}

export function isMarkdownPreviewPath(path: string): boolean {
    return path.startsWith(MARKDOWN_PREVIEW_PREFIX)
}

export function markdownPreviewSourcePath(path: string): string | null {
    if (!isMarkdownPreviewPath(path)) return null
    try {
        return decodeURIComponent(path.slice(MARKDOWN_PREVIEW_PREFIX.length))
    } catch {
        return null
    }
}

export function isMarkdownPreviewTab(tab: PreviewTabLike): boolean {
    return tab.kind === "markdown-preview" || isMarkdownPreviewPath(tab.path)
}

export function previewTabSourcePath(tab: PreviewTabLike): string | null {
    return tab.sourcePath ?? markdownPreviewSourcePath(tab.path)
}

export function isMarkdownPreviewForSource(tab: PreviewTabLike, sourcePath: string): boolean {
    return isMarkdownPreviewTab(tab) && previewTabSourcePath(tab) === sourcePath
}

export function isFileTab(tab: PreviewTabLike): boolean {
    if (tab.kind === "preview" || tab.kind === "markdown-preview" || tab.kind === "herdr-terminal") {
        return false
    }
    return !tab.path.startsWith("yuzora://")
}

export function markdownPreviewTabName(): string {
    return MARKDOWN_PREVIEW_TAB_NAME
}
