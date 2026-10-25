import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, expect, it, vi } from "vitest"
import { createInstance } from "i18next"
import { I18nextProvider } from "react-i18next"
import { Command, CommandList } from "@/components/ui/command"
import type { SearchEvent } from "@/lib/types"
import en from "@/lib/i18n/locales/en/menus.json"
import zh from "@/lib/i18n/locales/zh-TW/menus.json"
import { WorkspaceSearchGroup } from "./WorkspaceSearchGroup"

afterEach(cleanup)

it.each([
    ["en", "Could not finish searching. Results may be incomplete."],
    ["zh-TW", "無法完成搜尋，已找到的結果可能不完整。"],
])("shows a %s failure without hiding partial results or claiming no results", async (lng, notice) => {
    const i18n = createInstance()
    await i18n.init({ lng, fallbackLng: "en", resources: { en: { menus: en }, "zh-TW": { menus: zh } }, interpolation: { escapeValue: false } })
    const onReveal = vi.fn()
    function view(events: SearchEvent[], error: string | null) {
        return <I18nextProvider i18n={i18n}><Command shouldFilter={false}><CommandList>
            <WorkspaceSearchGroup events={events} query="needle" loading={false} error={error} caseSensitive={false} onToggleCaseSensitive={() => {}} onReveal={onReveal} />
        </CommandList></Command></I18nextProvider>
    }
    const rendered = render(view([{ type: "match", path: "/fixture/partial.txt", matches: [{ line: 7, col: 0, preview: "needle" }] }], "channel unavailable"))
    expect(screen.getByRole("alert")).toHaveTextContent(notice)
    expect(screen.getByRole("alert")).toHaveTextContent("channel unavailable")
    fireEvent.click(screen.getByRole("option"))
    expect(onReveal).toHaveBeenCalledWith("/fixture/partial.txt", 7)
    rendered.rerender(view([{ type: "done", truncated: false, fileCount: 0 }], ""))
    expect(screen.getByRole("alert")).toHaveTextContent(notice)
    expect(screen.queryByText(i18n.t("workspaceSearch.noResults", { ns: "menus" }))).not.toBeInTheDocument()
    rendered.rerender(view([{ type: "done", truncated: false, fileCount: 0 }], null))
    expect(screen.queryByRole("alert")).not.toBeInTheDocument()
    expect(screen.getByText(i18n.t("workspaceSearch.noResults", { ns: "menus" }))).toBeInTheDocument()
})

it.each([
    ["en", "Search results may be incomplete. Narrow your search."],
    ["zh-TW", "搜尋結果可能不完整，請縮小搜尋範圍。"],
])("shows the %s incomplete notice without disabling retained results", async (lng, notice) => {
    const i18n = createInstance()
    await i18n.init({ lng, fallbackLng: "en", resources: { en: { menus: en }, "zh-TW": { menus: zh } }, interpolation: { escapeValue: false } })
    const onReveal = vi.fn()
    function view(truncated: boolean) {
        const events: SearchEvent[] = [
            { type: "match", path: "/fixture/a.txt", matches: [{ line: 42, col: 0, preview: "needle line" }] },
            { type: "done", truncated, fileCount: 1 },
        ]
        return <I18nextProvider i18n={i18n}><Command shouldFilter={false}><CommandList>
            <WorkspaceSearchGroup events={events} query="needle" loading={false} caseSensitive={false} onToggleCaseSensitive={() => {}} onReveal={onReveal} />
        </CommandList></Command></I18nextProvider>
    }
    const rendered = render(view(true))
    expect(screen.getByText(notice)).toBeInTheDocument()
    fireEvent.click(screen.getByRole("option"))
    expect(onReveal).toHaveBeenCalledWith("/fixture/a.txt", 42)
    rendered.rerender(view(false))
    expect(screen.queryByText(notice)).not.toBeInTheDocument()
    expect(screen.getByRole("option")).toBeInTheDocument()
})

async function highlightView(preview: string, query: string, caseSensitive: boolean, onToggle = vi.fn()) {
    const i18n = createInstance()
    await i18n.init({ lng: "en", resources: { en: { menus: en } }, interpolation: { escapeValue: false } })
    const element = (sensitive: boolean, nextPreview = preview, nextQuery = query) => {
        const events: SearchEvent[] = [{ type: "match", path: "/fixture/a.txt", matches: [1, 2].map(line => ({ line, col: 0, preview: nextPreview })) }]
        return <I18nextProvider i18n={i18n}><Command shouldFilter={false}><CommandList>
            <WorkspaceSearchGroup events={events} query={nextQuery} loading={false} caseSensitive={sensitive} onToggleCaseSensitive={onToggle} onReveal={() => {}} />
        </CommandList></Command></I18nextProvider>
    }
    return { ...render(element(caseSensitive)), element }
}

function markedRows(preview: string) {
    const rows = screen.getAllByRole("option")
    expect(rows).toHaveLength(2)
    return rows.map(row => {
        const excerpt = row.querySelector("span.truncate")!
        expect(excerpt.textContent).toBe(preview)
        return [...excerpt.querySelectorAll("mark")].map(mark => mark.textContent)
    })
}

it.each([
    { preview: "İxx", query: "xx", sensitive: false, marks: ["xx"] },
    { preview: "😀İxx", query: "xx", sensitive: false, marks: ["xx"] },
    { preview: "ſs", query: "ss", sensitive: false, marks: ["ſs"] },
    { preview: "ς", query: "σ", sensitive: false, marks: ["ς"] },
    { preview: "Aa aA", query: "Aa", sensitive: true, marks: ["Aa"] },
    { preview: "Aa aA", query: "Aa", sensitive: false, marks: ["Aa", "aA"] },
    { preview: "axb a.b [x] \\", query: "a.b", sensitive: false, marks: ["a.b"] },
    { preview: "axb a.b [x] \\", query: "[x]", sensitive: false, marks: ["[x]"] },
    { preview: "axb a.b [x] \\", query: "\\", sensitive: false, marks: ["\\"] },
    { preview: "a .* b", query: ".*", sensitive: false, marks: [".*"] },
    { preview: "aaa", query: "aa", sensitive: false, marks: ["aa"] },
    { preview: "İ", query: "i", sensitive: false, marks: [] },
    { preview: "ß", query: "ss", sensitive: false, marks: [] },
    { preview: "no match", query: "needle", sensitive: false, marks: [] },
    { preview: "unchanged", query: "", sensitive: false, marks: [] },
])("highlights literal $query in $preview (case-sensitive=$sensitive) without changing text", async ({ preview, query, sensitive, marks }) => {
    await highlightView(preview, query, sensitive)
    expect(markedRows(preview)).toEqual([marks, marks])
})

it("updates marks and the case toggle when case sensitivity changes", async () => {
    const toggle = vi.fn()
    const preview = "Aa aA"
    const rendered = await highlightView(preview, "Aa", false, toggle)
    expect(markedRows(preview)).toEqual([["Aa", "aA"], ["Aa", "aA"]])
    const button = screen.getByRole("button", { name: en.workspaceSearch.matchCaseAriaLabel })
    expect(button).toHaveAttribute("aria-pressed", "false")
    fireEvent.click(button)
    expect(toggle).toHaveBeenCalledOnce()
    rendered.rerender(rendered.element(true))
    expect(button).toHaveAttribute("aria-pressed", "true")
    expect(markedRows(preview)).toEqual([["Aa"], ["Aa"]])
})

it("leaves short previews intact when a pasted literal query is much longer", async () => {
    const preview = "short preview"
    await highlightView(preview, "a".repeat(200_000), false)
    expect(markedRows(preview)).toEqual([[], []])
})


it("refreshes the shared pattern when streamed previews or the query change", async () => {
    const rendered = await highlightView("a", "needle", false)
    expect(markedRows("a")).toEqual([[], []])
    rendered.rerender(rendered.element(false, "needle"))
    expect(markedRows("needle")).toEqual([["needle"], ["needle"]])
    rendered.rerender(rendered.element(false, "needle", "a".repeat(200_000)))
    expect(markedRows("needle")).toEqual([[], []])
    rendered.rerender(rendered.element(false, "needle", "needle"))
    expect(markedRows("needle")).toEqual([["needle"], ["needle"]])
})
