import { afterEach, expect, it } from "vitest"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"

import { logQuery, logSources, logExport, getLogLevel, setLogLevel } from "./logQuery"
import type { LogQueryFilters } from "./logQuery"
import type { LogRecord } from "@/lib/types"

afterEach(() => clearMocks())

const sampleRecord: LogRecord = {
    timestamp: "2026-07-07T00:00:00Z",
    level: "info",
    kind: "user_action",
    source: "ui",
    workspace_path: null,
    event: "click",
    message: "clicked button",
    metadata: {}
}

it("logQuery forwards a fully populated filters object and returns the records", async () => {
    const filters: LogQueryFilters = {
        since: "2026-01-01",
        until: "2026-02-01",
        levels: ["info", "error"],
        kinds: ["user_action"],
        sources: ["ui"],
        text: "click",
        limit: 50
    }
    mockIPC((cmd, payload) => {
        expect(cmd).toBe("log_query")
        expect(payload).toEqual({ filters })
        return [sampleRecord]
    })
    const records = await logQuery(filters)
    expect(records).toEqual([sampleRecord])
})

it("logQuery sends an empty filters object as-is and can return an empty result", async () => {
    mockIPC((cmd, payload) => {
        expect(cmd).toBe("log_query")
        expect(payload).toEqual({ filters: {} })
        return []
    })
    const records = await logQuery({})
    expect(records).toEqual([])
})

it("logQuery leaves unset optional filter fields absent from the payload", async () => {
    mockIPC((_cmd, payload) => {
        const f = (payload as { filters: LogQueryFilters }).filters
        expect("since" in f).toBe(false)
        expect("until" in f).toBe(false)
        expect("limit" in f).toBe(false)
        expect(f.text).toBe("err")
        return []
    })
    await logQuery({ text: "err" })
})

it("logQuery forwards explicit undefined values unchanged (no sanitization)", async () => {
    mockIPC((_cmd, payload) => {
        const f = (payload as { filters: LogQueryFilters }).filters
        expect("since" in f).toBe(true)
        expect(f.since).toBeUndefined()
        expect(f.limit).toBe(10)
        return []
    })
    await logQuery({ since: undefined, limit: 10 })
})

it("logQuery rejects when invoke rejects", async () => {
    mockIPC(() => {
        throw new Error("log_query boom")
    })
    await expect(logQuery({})).rejects.toThrow("log_query boom")
})

it("logSources returns the source list", async () => {
    mockIPC((cmd) => {
        expect(cmd).toBe("log_sources")
        return ["ui", "backend"]
    })
    expect(await logSources()).toEqual(["ui", "backend"])
})

it("logSources returns an empty list when there are no sources", async () => {
    mockIPC(() => [])
    expect(await logSources()).toEqual([])
})

it("logSources rejects when invoke rejects", async () => {
    mockIPC(() => {
        throw new Error("log_sources boom")
    })
    await expect(logSources()).rejects.toThrow("log_sources boom")
})

it("logExport forwards dest and sanitize:true and returns the export path", async () => {
    mockIPC((cmd, payload) => {
        expect(cmd).toBe("log_export")
        expect(payload).toEqual({ dest: "/tmp/log.jsonl", sanitize: true })
        return "/tmp/log.jsonl"
    })
    expect(await logExport("/tmp/log.jsonl", true)).toBe("/tmp/log.jsonl")
})

it("logExport forwards sanitize:false", async () => {
    mockIPC((_cmd, payload) => {
        expect(payload).toEqual({ dest: "/tmp/log.jsonl", sanitize: false })
        return "/tmp/log.jsonl"
    })
    await logExport("/tmp/log.jsonl", false)
})

it("logExport rejects when invoke rejects", async () => {
    mockIPC(() => {
        throw new Error("log_export boom")
    })
    await expect(logExport("/tmp/log.jsonl", true)).rejects.toThrow("log_export boom")
})

it("getLogLevel calls get_log_level", async () => {
    mockIPC((cmd) => {
        expect(cmd).toBe("get_log_level")
        return "info"
    })
    expect(await getLogLevel()).toBe("info")
})

it("setLogLevel passes the level arg", async () => {
    mockIPC((cmd, payload) => {
        expect(cmd).toBe("set_log_level")
        expect(payload).toEqual({ level: "debug" })
    })
    await setLogLevel("debug")
})
