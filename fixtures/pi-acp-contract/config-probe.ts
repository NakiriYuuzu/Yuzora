// Soak 回饋修復的 host 面 E2E gate（2026-07-22）：
//   #1 模型切換（wire 生效＋currentValue 更新）
//   #2 thinking max（動態清單驗證）
//   #3 usage_update（context/pricing 通道）
//   #4 steering（turn 進行中送第二句、兩個 request 都收斂）
//   #6 內建 slash commands（/session、/name＋availableCommands 併入）
// 護欄：pi 的 setModel/setThinkingLevel 會寫全域 ~/.pi/agent/settings.json——
// 執行前備份、結束（含失敗）還原（鏡射 record.ts）。
// 用法（adapter 路徑要絕對）：
//   bun fixtures/pi-acp-contract/config-probe.ts --command "node $PWD/adapters/yuzora-pi-acp/dist/index.mjs"
import { mkdtemp, readFile, writeFile } from "node:fs/promises"
import { homedir, tmpdir } from "node:os"
import { join } from "node:path"

import { AcpDriver } from "./driver"

function asRecord(value: unknown): Record<string, unknown> {
    return value && typeof value === "object" ? value as Record<string, unknown> : {}
}

const checks: { name: string; pass: boolean; detail?: string }[] = []
function check(name: string, pass: boolean, detail?: string) {
    checks.push({ name, pass, ...(detail ? { detail } : {}) })
    console.log(`${pass ? "PASS" : "FAIL"} ${name}${detail ? ` — ${detail}` : ""}`)
}

const CLIENT_CAPS = {
    fs: { readTextFile: true, writeTextFile: true },
    terminal: true,
    session: { configOptions: { boolean: {} } },
    elicitation: { form: {} }
}

function configOption(result: Record<string, unknown>, id: string): Record<string, unknown> | undefined {
    const options = Array.isArray(result.configOptions) ? result.configOptions.map(asRecord) : []
    return options.find((option) => option.id === id)
}

function agentText(driver: AcpDriver): string {
    return driver.lines
        .filter((line) => line.dir === "a2c")
        .map((line) => {
            const update = asRecord(asRecord(asRecord(line.msg).params).update)
            if (update.sessionUpdate !== "agent_message_chunk") return ""
            const content = asRecord(update.content)
            return typeof content.text === "string" ? content.text : ""
        })
        .join("")
}

async function main() {
    const commandFlagIndex = process.argv.indexOf("--command")
    const command = commandFlagIndex !== -1
        ? process.argv[commandFlagIndex + 1]
        : "node adapters/yuzora-pi-acp/dist/index.mjs"
    const cwd = await mkdtemp(join(tmpdir(), "yuzora-config-probe-"))

    const piSettingsPath = join(homedir(), ".pi", "agent", "settings.json")
    let piSettingsBackup: string | null = null
    try {
        piSettingsBackup = await readFile(piSettingsPath, "utf8")
        console.log("pi settings backed up")
    } catch {
        piSettingsBackup = null
    }

    const driver = new AcpDriver(command, cwd)
    try {
        await driver.request("initialize", { protocolVersion: 1, clientCapabilities: CLIENT_CAPS }, 120_000)
        const created = asRecord(await driver.request("session/new", { cwd, mcpServers: [] }, 180_000))
        const sessionId = created.sessionId

        // --- #6 availableCommands 含 builtin ---
        await driver.waitForNotification((msg) => {
            const update = asRecord(asRecord(msg.params).update)
            return update.sessionUpdate === "available_commands_update"
        }, 30_000)
        const commandsUpdate = driver.lines
            .map((line) => asRecord(asRecord(asRecord(line.msg).params).update))
            .find((update) => update.sessionUpdate === "available_commands_update")
        const commandNames = (Array.isArray(commandsUpdate?.availableCommands) ? commandsUpdate.availableCommands : [])
            .map((entry) => String(asRecord(entry).name))
        const expectedBuiltins = ["compact", "session", "name", "steering", "follow-up", "autocompact", "export", "changelog"]
        check(
            "available commands include the builtin set",
            expectedBuiltins.every((name) => commandNames.includes(name)),
            expectedBuiltins.filter((name) => !commandNames.includes(name)).join(",") || "all present"
        )

        // --- #1 model switch（優先切到 gpt-5.6-sol——使用者實際情境的模型） ---
        const initialModel = configOption(created, "model")
        const modelValues = (Array.isArray(initialModel?.options) ? initialModel.options : [])
            .map((option) => String(asRecord(option).value))
        const target = modelValues.find((value) => value === "yuuzu-router/gpt-5.6-sol")
            ?? modelValues.find((value) => value !== initialModel?.currentValue)
        const modelResult = asRecord(await driver.request("session/set_config_option", {
            sessionId, configId: "model", value: target
        }, 60_000))
        check(
            "model switch takes effect in the returned config",
            configOption(modelResult, "model")?.currentValue === target,
            `${String(initialModel?.currentValue)} -> ${String(configOption(modelResult, "model")?.currentValue)}`
        )

        // --- #2 thinking max（切到支援 max 的模型後設定；SDK clamp 語意——
        //     不得拋 -32602，currentValue 為實際生效值） ---
        const thinkingResult = asRecord(await driver.request("session/set_config_option", {
            sessionId, configId: "thought_level", value: "max"
        }, 60_000).catch((error) => ({ error: String(error) })))
        const effectiveThinking = configOption(thinkingResult, "thought_level")?.currentValue
        check(
            "thinking max is accepted (clamped to the model's effective level)",
            thinkingResult.error === undefined && typeof effectiveThinking === "string",
            thinkingResult.error !== undefined ? String(thinkingResult.error) : `effective=${String(effectiveThinking)}`
        )

        // --- #4 steering＋#3 usage：長 turn 中送第二句 ---
        const firstPrompt = driver.request("session/prompt", {
            sessionId,
            prompt: [{ type: "text", text: "Count from 1 to 20, one number per line. Do not stop early." }]
        }, 300_000)
        // 等第一個 turn 開始（running info）再 steer。
        await driver.waitForNotification((msg) => {
            const update = asRecord(asRecord(msg.params).update)
            return update.sessionUpdate === "session_info_update"
                && asRecord(asRecord(update._meta).piAcp).running === true
        }, 60_000)
        const steerPrompt = driver.request("session/prompt", {
            sessionId,
            prompt: [{ type: "text", text: "After the numbers, also write the exact word STEERED-OK." }]
        }, 300_000)
        const [firstResult, steerResult] = await Promise.all([firstPrompt, steerPrompt])
        check("primary turn ends normally", asRecord(firstResult).stopReason === "end_turn", String(asRecord(firstResult).stopReason))
        check("steered prompt resolves without hanging", asRecord(steerResult).stopReason === "end_turn", String(asRecord(steerResult).stopReason))
        check(
            "steered instruction reached the running turn",
            agentText(driver).includes("STEERED-OK"),
            agentText(driver).includes("STEERED-OK") ? "marker found" : "marker missing"
        )
        const usageUpdate = driver.lines
            .map((line) => asRecord(asRecord(asRecord(line.msg).params).update))
            .find((update) => update.sessionUpdate === "usage_update")
        check(
            "usage_update reports context tokens and window",
            usageUpdate !== undefined
                && Number.isFinite(usageUpdate.used)
                && Number.isFinite(usageUpdate.size),
            usageUpdate ? `used=${String(usageUpdate.used)} size=${String(usageUpdate.size)}` : "no usage_update seen"
        )

        // --- #6 /session＋/name ---
        const before = driver.lines.length
        const sessionCmd = asRecord(await driver.request("session/prompt", {
            sessionId,
            prompt: [{ type: "text", text: "/session" }]
        }, 60_000))
        const sessionText = driver.lines.slice(before)
            .map((line) => JSON.stringify(asRecord(asRecord(line.msg).params).update ?? {}))
            .join("\n")
        check("/session ends the turn without an LLM call", asRecord(sessionCmd).stopReason === "end_turn", String(asRecord(sessionCmd).stopReason))
        check(
            "/session prints stats",
            sessionText.includes("Session:") && sessionText.includes("Tokens:") && sessionText.includes("Cost:"),
            sessionText.includes("Session:") ? "stats found" : "no stats output"
        )
        await driver.request("session/prompt", {
            sessionId,
            prompt: [{ type: "text", text: "/name probe-named-session" }]
        }, 60_000)
        const namedInfo = driver.lines
            .map((line) => asRecord(asRecord(asRecord(line.msg).params).update))
            .find((update) => update.sessionUpdate === "session_info_update" && update.title === "probe-named-session")
        check("/name updates the session title", namedInfo !== undefined)
    } finally {
        await driver.close()
        if (piSettingsBackup !== null) {
            await writeFile(piSettingsPath, piSettingsBackup, "utf8")
            console.log("pi settings restored")
        }
    }
    const failed = checks.filter((entry) => !entry.pass)
    console.log(failed.length === 0 ? `ALL ${checks.length} CHECKS PASSED` : `${failed.length} CHECKS FAILED`)
    process.exit(failed.length === 0 ? 0 : 1)
}

main().catch((error) => {
    console.error(error)
    process.exit(1)
})
