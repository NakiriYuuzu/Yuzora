// The folder row whose expanded list holds `row`; null at the workspace root.
export function containingFolderRow(row: HTMLElement): HTMLElement | null {
    return row.closest("li")?.parentElement?.closest("li")
        ?.querySelector<HTMLElement>(':scope > div > [data-tree-dir="true"]') ?? null
}
