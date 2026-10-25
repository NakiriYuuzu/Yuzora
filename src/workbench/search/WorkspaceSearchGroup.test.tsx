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
