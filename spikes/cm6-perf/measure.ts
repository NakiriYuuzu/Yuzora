import { createServer } from "vite"
import { chromium, type Browser, type CDPSession } from "playwright"
import { mkdirSync, writeFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

const __dirname = dirname(fileURLToPath(import.meta.url))
const BASE_URL = "http://localhost:5299"
const RESULTS_DIR = join(__dirname, "results")

interface CaseDef {
    fixture: string
    syntaxOff: boolean
    label: string
    timeoutMs: number
}

const CASES: CaseDef[] = [
    { fixture: "f-1mb.ts", syntaxOff: false, label: "f-1mb.ts (syntax on)", timeoutMs: 20_000 },
    { fixture: "f-10mb.ts", syntaxOff: false, label: "f-10mb.ts (syntax on)", timeoutMs: 30_000 },
    { fixture: "f-30mb.ts", syntaxOff: false, label: "f-30mb.ts (syntax on)", timeoutMs: 60_000 },
    { fixture: "f-50mb.ts", syntaxOff: false, label: "f-50mb.ts (syntax on)", timeoutMs: 60_000 },
    { fixture: "f-80mb.ts", syntaxOff: false, label: "f-80mb.ts (syntax on)", timeoutMs: 60_000 },
    { fixture: "l-10k.ts", syntaxOff: false, label: "l-10k.ts (syntax on)", timeoutMs: 20_000 },
    { fixture: "l-10k.ts", syntaxOff: true, label: "l-10k.ts (syntax off)", timeoutMs: 20_000 },
    { fixture: "l-100k.ts", syntaxOff: false, label: "l-100k.ts (syntax on)", timeoutMs: 30_000 },
    { fixture: "l-100k.ts", syntaxOff: true, label: "l-100k.ts (syntax off)", timeoutMs: 30_000 },
    { fixture: "l-1m.ts", syntaxOff: false, label: "l-1m.ts (syntax on)", timeoutMs: 45_000 },
    { fixture: "l-1m.ts", syntaxOff: true, label: "l-1m.ts (syntax off)", timeoutMs: 45_000 },
]

const TYPE_TEXT = "function measureTypingLatencyInline() { return 42 }"

interface CaseResult {
    label: string
    fixture: string
    syntaxOff: boolean
    status: "ok" | "fail"
    reason?: string
    metrics?: {
        chars: number
        lines: number
        fetchMs: number
        stateCreateMs: number
        viewCreateMs: number
        firstPaintMs: number
    }
    typing?: {
        samples: number
        p50: number | null
        p95: number | null
        max: number | null
    }
    scroll?: {
        ms: number
        longTaskCountDuring: number
        longTaskDurationDuring: number
    }
    longTasksTotal?: {
        count: number
        totalDurationMs: number
    }
    memory?: {
        afterInitMB: number | null
        afterTypeMB: number | null
        afterScrollMB: number | null
        afterGcMB: number | null
    }
    wallMs: number
}

function percentile(values: number[], p: number): number | null {
    if (values.length === 0) return null
    const sorted = [...values].sort((a, b) => a - b)
    const idx = Math.min(sorted.length - 1, Math.floor((p / 100) * sorted.length))
    return sorted[idx] ?? null
}

async function readHeapMB(client: CDPSession): Promise<number | null> {
    try {
        const { metrics } = await client.send("Performance.getMetrics")
        const m = metrics.find((x) => x.name === "JSHeapUsedSize")
        return m ? m.value / (1024 * 1024) : null
    } catch {
        return null
    }
}

async function runCase(browser: Browser, def: CaseDef): Promise<CaseResult> {
    const start = performance.now()
    const context = await browser.newContext({ viewport: { width: 1280, height: 800 } })
    const page = await context.newPage()
    const consoleErrors: string[] = []
    page.on("pageerror", (err) => consoleErrors.push(String(err)))
    page.on("console", (msg) => {
        if (msg.type() === "error") consoleErrors.push(msg.text())
    })

    let crashed = false
    page.once("crash", () => {
        crashed = true
    })

    try {
        const client = await context.newCDPSession(page)
        await client.send("Performance.enable")
        await client.send("HeapProfiler.enable")

        const url = `${BASE_URL}/?fixture=${encodeURIComponent(def.fixture)}&syntaxOff=${def.syntaxOff ? "1" : "0"}`
        await page.goto(url, { timeout: def.timeoutMs })
        await page.waitForFunction(() => (window as any).cm6Perf?.ready === true, undefined, {
            timeout: def.timeoutMs,
        })
        if (crashed) throw new Error("renderer crashed")

        const pageError: string | null = await page.evaluate(() => (window as any).cm6Perf.error)
        if (pageError) throw new Error(`in-page error: ${pageError}`)

        const metrics = await page.evaluate(() => (window as any).cm6Perf.metrics)
        const afterInitMB = await readHeapMB(client)

        await page.waitForTimeout(300)

        await page.evaluate(() => (window as any).cm6Perf.focusMiddle())
        await page.waitForTimeout(50)
        await page.evaluate(() => (window as any).cm6Perf.clearKeyLatencies())
        await page.keyboard.type(TYPE_TEXT, { delay: 30 })
        await page.waitForTimeout(400)
        const keyLatencies: number[] = await page.evaluate(() => (window as any).cm6Perf.keyLatencies)
        const afterTypeMB = await readHeapMB(client)

        const scroll = await page.evaluate(() => (window as any).cm6Perf.scrollThrough(40, 1200))
        await page.waitForTimeout(200)
        const afterScrollMB = await readHeapMB(client)

        const rawLongTasks: { start: number; duration: number }[] = await page.evaluate(
            () => (window as any).cm6Perf.longTasks,
        )

        await client.send("HeapProfiler.collectGarbage")
        await page.waitForTimeout(100)
        const afterGcMB = await readHeapMB(client)

        if (crashed) throw new Error("renderer crashed")

        return {
            label: def.label,
            fixture: def.fixture,
            syntaxOff: def.syntaxOff,
            status: "ok",
            metrics,
            typing: {
                samples: keyLatencies.length,
                p50: percentile(keyLatencies, 50),
                p95: percentile(keyLatencies, 95),
                max: keyLatencies.length ? Math.max(...keyLatencies) : null,
            },
            scroll,
            longTasksTotal: {
                count: rawLongTasks.length,
                totalDurationMs: rawLongTasks.reduce((a, b) => a + b.duration, 0),
            },
            memory: { afterInitMB, afterTypeMB, afterScrollMB, afterGcMB },
            wallMs: performance.now() - start,
        }
    } catch (err) {
        const reason = `${(err as Error).message}${crashed ? " [renderer crashed]" : ""}${
            consoleErrors.length ? " | console: " + consoleErrors.slice(0, 3).join("; ") : ""
        }`
        return {
            label: def.label,
            fixture: def.fixture,
            syntaxOff: def.syntaxOff,
            status: "fail",
            reason,
            wallMs: performance.now() - start,
        }
    } finally {
        await page.close().catch(() => {})
        await context.close().catch(() => {})
    }
}

async function main() {
    mkdirSync(RESULTS_DIR, { recursive: true })

    const server = await createServer({
        configFile: join(__dirname, "vite.config.ts"),
        root: __dirname,
    })
    await server.listen()
    console.log(`vite dev server up on ${BASE_URL}`)

    let browser = await chromium.launch({ headless: true })

    console.log("warmup pass (triggers vite dep pre-bundling, excluded from results)...")
    await runCase(browser, { fixture: "f-1mb.ts", syntaxOff: false, label: "warmup", timeoutMs: 30_000 })

    const results: CaseResult[] = []
    try {
        for (const def of CASES) {
            if (!browser.isConnected()) {
                console.log("browser disconnected, relaunching...")
                browser = await chromium.launch({ headless: true })
            }
            console.log(`running: ${def.label}`)
            const result = await runCase(browser, def)
            if (result.status === "ok") {
                console.log(
                    `  ok  init=${result.metrics?.viewCreateMs.toFixed(1)}ms ` +
                        `typing(n=${result.typing?.samples}) p50=${result.typing?.p50?.toFixed(1)}ms p95=${result.typing?.p95?.toFixed(1)}ms ` +
                        `heap(gc)=${result.memory?.afterGcMB?.toFixed(1)}MB`,
                )
            } else {
                console.log(`  FAIL  ${result.reason}`)
            }
            results.push(result)
        }
    } finally {
        await browser.close().catch(() => {})
        await server.close().catch(() => {})
    }

    const rawPath = join(RESULTS_DIR, "raw.json")
    writeFileSync(rawPath, JSON.stringify(results, null, 2))
    console.log(`\nraw results written to ${rawPath}`)

    console.log("\n--- summary table ---")
    for (const r of results) {
        if (r.status === "fail") {
            console.log(`${r.label}: FAIL (${r.reason})`)
            continue
        }
        console.log(
            `${r.label}: chars=${r.metrics?.chars} lines=${r.metrics?.lines} ` +
                `init=${r.metrics?.viewCreateMs.toFixed(1)}ms firstPaint=${r.metrics?.firstPaintMs.toFixed(1)}ms ` +
                `typing(n=${r.typing?.samples}) p50=${r.typing?.p50?.toFixed(1)}ms p95=${r.typing?.p95?.toFixed(1)}ms ` +
                `scrollLongTasks=${r.scroll?.longTaskCountDuring}(${r.scroll?.longTaskDurationDuring.toFixed(1)}ms) ` +
                `totalLongTasks=${r.longTasksTotal?.count} heapAfterGC=${r.memory?.afterGcMB?.toFixed(1)}MB`,
        )
    }
}

main().catch((err) => {
    console.error(err)
    process.exit(1)
})
