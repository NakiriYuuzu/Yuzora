// P1 錄音器：對真實 adapter（預設 pin 的 pi-acp）跑固定情境，錄下完整 ACP wire
// 對話為 contract fixtures。用法：
//   bun fixtures/pi-acp-contract/record.ts [--command "bunx pi-acp@0.0.31"] [--out DIR] [--only 情境名]
// 情境：initialize / session-prompt-write / session-cancel / session-load（load
// 依賴 session-prompt-write 產生的 sessionId，--only 時可用 --session-id 補）。
// 注意：會實際呼叫模型（錄音時盡量切 fast model＋低 thinking）；session 檔會
// 落在使用者的 pi session store（cwd 為 /tmp 暫存 workspace，不污染 repo）。
import { mkdtemp, mkdir, readFile, writeFile } from "node:fs/promises"
import { homedir, tmpdir } from "node:os"
import { join } from "node:path"

import { AcpDriver } from "./driver"
import { invariants, parseRecording } from "./contract"

const DEFAULT_COMMAND = "bunx pi-acp@0.0.31"
const DEFAULT_OUT = "fixtures/pi-acp-contract/recordings"

interface CliOptions {
    command: string
    out: string
    only?: string
    sessionId?: string
}

function parseArgs(argv: string[]): CliOptions {
    const options: CliOptions = { command: DEFAULT_COMMAND, out: DEFAULT_OUT }
    for (let index = 0; index < argv.length; index += 1) {
        const flag = argv[index]
        const value = argv[index + 1]
        if (flag === "--command" && value) { options.command = value; index += 1 }
        else if (flag === "--out" && value) { options.out = value; index += 1 }
        else if (flag === "--only" && value) { options.only = value; index += 1 }
        else if (flag === "--session-id" && value) { options.sessionId = value; index += 1 }
        else throw new Error(`unknown argument: ${flag}`)
    }
    return options
}

function asRecord(value: unknown): Record<string, unknown> {
    return value && typeof value === "object" ? value as Record<string, unknown> : {}
}

// 鏡射 yuzora acpConnection 的 initialize 參數（逐字）。
function initializeParams(): Record<string, unknown> {
    return {
        protocolVersion: 1,
        clientCapabilities: {
            fs: { readTextFile: true, writeTextFile: true },
            terminal: true,
            session: { configOptions: { boolean: {} } }
        }
    }
}

async function saveRecording(
    outDir: string,
    scenario: string,
    driver: AcpDriver,
    extraMeta: Record<string, unknown>,
    startedAtIso: string
) {
    const meta = {
        harness: "pi-acp-contract/1",
        scenario,
        recordedAt: startedAtIso,
        command: driver.command,
        cwd: driver.cwd,
        piAcpCommandEnv: process.env.PI_ACP_PI_COMMAND ?? null,
        ...extraMeta
    }
    const rows = [
        JSON.stringify({ meta }),
        ...driver.lines.map((line) => JSON.stringify(line)),
        JSON.stringify({ trailer: { stderrTail: driver.stderrTail() } })
    ]
    const path = join(outDir, `${scenario}.jsonl`)
    await writeFile(path, `${rows.join("\n")}\n`, "utf8")

    const findings = invariants(parseRecording(rows.join("\n")))
    const errors = findings.filter((finding) => finding.level === "error")
    for (const finding of findings) {
        console.log(`  [${finding.level}] ${finding.message}`)
    }
    console.log(`  saved ${path} (${driver.lines.length} wire messages, invariant errors: ${errors.length})`)
    if (errors.length > 0) process.exitCode = 1
}

// 從 session/new 結果的 configOptions 把 thinking 壓到最低（錄音求快求省；同時
// 就是 set_config_option 的 contract 覆蓋）。刻意不動 model：pi 的 setModel 會把
// 選擇存進全域 settings，挑到 catalog 解析不了的動態模型（如 router 的 -fast
// 變體）會讓同輪後續情境的新 session 直接 auth 失敗。
async function tuneSessionConfig(driver: AcpDriver, sessionId: string, configOptions: unknown) {
    const options = Array.isArray(configOptions) ? configOptions.map(asRecord) : []
    const flatten = (option: Record<string, unknown>): Record<string, unknown>[] => {
        const nested = Array.isArray(option.options) ? option.options.map(asRecord) : []
        return nested.flatMap((item) => Array.isArray(item.options) ? item.options.map(asRecord) : [item])
    }
    const thought = options.find((option) => option.category === "thought_level" && option.type === "select")
    if (thought) {
        const values = flatten(thought).map((value) => String(value.value))
        const low = ["off", "minimal", "low"].find((candidate) => values.includes(candidate))
        if (low && low !== thought.currentValue) {
            await driver.request("session/set_config_option", {
                sessionId, configId: thought.id, value: low
            }, 20_000)
        }
    }
}

async function main() {
    const options = parseArgs(process.argv.slice(2))
    await mkdir(options.out, { recursive: true })
    const workspace = await mkdtemp(join(tmpdir(), "yuzora-pi-acp-contract-"))
    const startedAtIso = new Date().toISOString()
    console.log(`workspace: ${workspace}`)
    console.log(`command:   ${options.command}`)

    // 防污染護欄：pi 的 setModel/setThinkingLevel 語意是「存 session＋全域
    // settings」——tuner 的 set_config 會改寫使用者的 defaultModel／
    // defaultThinkingLevel。錄音前備份、結束（含失敗）還原。
    const piSettingsPath = join(homedir(), ".pi", "agent", "settings.json")
    let piSettingsBackup: string | null = null
    try {
        piSettingsBackup = await readFile(piSettingsPath, "utf8")
    } catch {
        piSettingsBackup = null
    }
    const restorePiSettings = async () => {
        if (piSettingsBackup === null) return
        await writeFile(piSettingsPath, piSettingsBackup, "utf8")
        console.log("pi settings restored")
    }
    try {
        await recordScenarios(options, workspace, startedAtIso)
    } finally {
        await restorePiSettings()
    }
}

async function recordScenarios(options: CliOptions, workspace: string, startedAtIso: string) {

    const state: { sessionId?: string } = { sessionId: options.sessionId }
    const wants = (scenario: string) => !options.only || options.only === scenario

    if (wants("initialize")) {
        console.log("scenario: initialize")
        const driver = new AcpDriver(options.command, workspace)
        try {
            const result = await driver.request("initialize", initializeParams(), 60_000)
            await saveRecording(options.out, "initialize", driver, {
                agentInfo: asRecord(result).agentInfo ?? null
            }, startedAtIso)
        } finally {
            await driver.close()
        }
    }

    if (wants("session-prompt-write")) {
        console.log("scenario: session-prompt-write")
        const driver = new AcpDriver(options.command, workspace)
        try {
            const init = await driver.request("initialize", initializeParams(), 60_000)
            const created = asRecord(await driver.request("session/new", { cwd: workspace, mcpServers: [] }, 120_000))
            const sessionId = String(created.sessionId)
            state.sessionId = sessionId
            // configOptions 可能不在 session/new 結果、而是隨後以 config_option_update
            // 通知送達——兩處都試，反映 adapter 實際通道。
            let configOptions = created.configOptions
            if (!Array.isArray(configOptions) || configOptions.length === 0) {
                const update = await driver.waitForNotification((msg) => {
                    if (msg.method !== "session/update") return false
                    return asRecord(asRecord(msg.params).update).sessionUpdate === "config_option_update"
                }, 5_000).catch(() => null)
                if (update) {
                    configOptions = asRecord(asRecord(asRecord(update).params).update).configOptions
                }
            }
            await tuneSessionConfig(driver, sessionId, configOptions)
            const response = await driver.request("session/prompt", {
                sessionId,
                prompt: [{
                    type: "text",
                    text: "Use your write tool to create a file named hello.txt whose content is exactly: hello\nDo nothing else. Then reply with the single word: done"
                }]
            }, 300_000)
            await saveRecording(options.out, "session-prompt-write", driver, {
                agentInfo: asRecord(init).agentInfo ?? null,
                sessionId,
                stopReason: asRecord(response).stopReason ?? null
            }, startedAtIso)
        } finally {
            await driver.close()
        }
    }

    if (wants("session-cancel")) {
        console.log("scenario: session-cancel")
        const driver = new AcpDriver(options.command, workspace)
        try {
            await driver.request("initialize", initializeParams(), 60_000)
            const created = asRecord(await driver.request("session/new", { cwd: workspace, mcpServers: [] }, 120_000))
            const sessionId = String(created.sessionId)
            const promptDone = driver.request("session/prompt", {
                sessionId,
                prompt: [{
                    type: "text",
                    text: "Write a very long, slow, detailed essay about the history of mathematics, at least 3000 words, taking your time."
                }]
            }, 300_000)
            await driver.waitForNotification((msg) => {
                if (msg.method !== "session/update") return false
                const update = asRecord(asRecord(msg.params).update)
                return update.sessionUpdate === "agent_message_chunk"
                    || update.sessionUpdate === "agent_thought_chunk"
                    || update.sessionUpdate === "tool_call"
            }, 120_000)
            driver.notify("session/cancel", { sessionId })
            const response = await promptDone.catch((error: Error) => ({ promptError: error.message }))
            await saveRecording(options.out, "session-cancel", driver, {
                sessionId,
                stopReason: asRecord(response).stopReason ?? null
            }, startedAtIso)
        } finally {
            await driver.close()
        }
    }

    if (wants("session-load")) {
        if (!state.sessionId) {
            console.log("scenario: session-load skipped（無 sessionId——先跑 session-prompt-write 或用 --session-id）")
        } else {
            console.log("scenario: session-load")
            const driver = new AcpDriver(options.command, workspace)
            try {
                const init = await driver.request("initialize", initializeParams(), 60_000)
                const capabilities = asRecord(asRecord(init).agentCapabilities)
                if (capabilities.loadSession !== true) {
                    console.log("  agent 未宣告 loadSession capability——照錄嘗試結果")
                }
                await driver.request("session/load", {
                    sessionId: state.sessionId, cwd: workspace, mcpServers: []
                }, 120_000).catch((error: Error) => ({ loadError: error.message }))
                await new Promise((resolve) => setTimeout(resolve, 800))
                await saveRecording(options.out, "session-load", driver, {
                    sessionId: state.sessionId
                }, startedAtIso)
            } finally {
                await driver.close()
            }
        }
    }

    console.log("done")
}

main().catch((error) => {
    console.error(error)
    process.exit(1)
})
