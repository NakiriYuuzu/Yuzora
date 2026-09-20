export interface ElementContext {
    url: string
    selector: string
    text: string
    html: string
    truncated: boolean
    width: number
    height: number
    styles: Record<string, string>
}

export function formatElementContext(context: ElementContext, source: string): string {
    const styles = Object.entries(context.styles).slice(0, 32).map(([name, value]) => `${name}: ${value}`).join(";\n")
    return [
        "Selected webpage element",
        `Source: ${source}`,
        `Selector: ${context.selector.slice(0, 2048)}`,
        `Size: ${context.width} × ${context.height}`,
        `Text: ${context.text.slice(0, 4096)}`,
        "HTML:", context.html.slice(0, 24576),
        ...(context.truncated ? ["[HTML truncated]"] : []),
        "Computed styles:", styles,
    ].join("\n").slice(0, 32768)
}

export function isElementContext(value: unknown): value is ElementContext {
    if (!value || typeof value !== "object") return false
    const candidate = value as Partial<ElementContext>
    return typeof candidate.url === "string" && typeof candidate.selector === "string"
        && typeof candidate.text === "string" && typeof candidate.html === "string"
        && typeof candidate.width === "number" && typeof candidate.height === "number"
        && !!candidate.styles && typeof candidate.styles === "object"
        && Object.values(candidate.styles).every(item => typeof item === "string")
}
