// P5 gate 驗證：雙 runtime（builtin yuzora-pi-acp ⇄ community pi-acp）。
// (1) 跨 runtime 續聊：兩邊寫同一個 pi session store（~/.pi/agent/sessions），
//     任一 runtime 建立的 session 另一 runtime 都能 session/load 續聊、
//     replay 出對方留下的歷史。
// (2) V4 並存：同 cwd 各一 session 並行 prompt，事件依 sessionId 分流無串音。
// 斷言刻意不依賴「模型執行 write tool」——fast model 對寫檔指令的服從度
// flaky，而 runtime 隔離與 store 互通不需要它（檔案並行互不干擾已由
// multi-session.ts 以單 runtime 驗證）。
// 用法（路徑要絕對；community 版本固定為 P1 基線的 0.0.31 以求決定性）：
//   PI_ACP_PI_COMMAND="$HOME/.local/bin/pi-no-question" bun fixtures/pi-acp-contract/cross-runtime.ts \
//     --builtin "node $PWD/adapters/yuzora-pi-acp/dist/index.mjs" \
//     --community "bunx pi-acp@0.0.31"
import { mkdtemp } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"

import { AcpDriver } from "./driver"

function asRecord(value: unknown): Record<string, unknown> {
    return value && typeof value === "object" ? value as Record<string, unknown> : {}
}

function flagValue(name: string, fallback: string): string {
    const index = process.argv.indexOf(name)
    return index !== -1 ? process.argv[index + 1] : fallback
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

// chunk 串流可能把 marker 切在任意位置——訊息文字要先串接再比對。
function messageText(driver: AcpDriver, kinds: string[]): string {
    return driver.lines
        .filter((line) => line.dir === "a2c")
        .map((line) => {
            const update = asRecord(asRecord(asRecord(line.msg).params).update)
            if (!kinds.includes(String(update.sessionUpdate))) return ""
            const content = asRecord(update.content)
            return typeof content.text === "string" ? content.text : ""
        })
        .join("")
}

function agentText(driver: AcpDriver): string {
    return messageText(driver, ["agent_message_chunk"])
}

async function newSessionWithMarker(driver: AcpDriver, cwd: string, marker: string) {
    await driver.request("initialize", { protocolVersion: 1, clientCapabilities: CLIENT_CAPS }, 120_000)
    const created = asRecord(await driver.request("session/new", { cwd, mcpServers: [] }, 180_000))
    const result = asRecord(await driver.request("session/prompt", {
        sessionId: created.sessionId,
        prompt: [{ type: "text", text: `Reply with exactly the text ${marker} and stop.` }]
    }, 300_000))
    return { sessionId: String(created.sessionId), stopReason: String(result.stopReason) }
}

// marker 出現在 user prompt 裡——session/load 的 replay（user_message_chunk）
// 必然帶出它，不依賴模型輸出形狀。
async function loadAndContinue(driver: AcpDriver, cwd: string, sessionId: string, expectHistory: string) {
    await driver.request("initialize", { protocolVersion: 1, clientCapabilities: CLIENT_CAPS }, 120_000)
    await driver.request("session/load", { sessionId, cwd, mcpServers: [] }, 180_000)
    const historyReplayed = messageText(driver, ["user_message_chunk", "agent_message_chunk"]).includes(expectHistory)
    const result = asRecord(await driver.request("session/prompt", {
        sessionId,
        prompt: [{ type: "text", text: "Reply with the single word CONTINUED and stop." }]
    }, 300_000))
    return { historyReplayed, stopReason: String(result.stopReason) }
}

async function main() {
    const builtinCommand = flagValue("--builtin", "")
    const communityCommand = flagValue("--community", "bunx pi-acp@0.0.31")
    if (!builtinCommand) {
        console.error("--builtin <command> is required (absolute path)")
        process.exit(1)
    }
    const cwd = await mkdtemp(join(tmpdir(), "yuzora-cross-runtime-"))

    // --- (1a) builtin 建立 → community 續聊 ---
    const builtinNew = new AcpDriver(builtinCommand, cwd)
    let builtinSessionId = ""
    try {
        const { sessionId, stopReason } = await newSessionWithMarker(builtinNew, cwd, "MARKER-FROM-BUILTIN")
        builtinSessionId = sessionId
        check("builtin new session prompt end_turn", stopReason === "end_turn", stopReason)
    } finally {
        await builtinNew.close()
    }

    const communityLoad = new AcpDriver(communityCommand, cwd)
    try {
        const { historyReplayed, stopReason } = await loadAndContinue(communityLoad, cwd, builtinSessionId, "MARKER-FROM-BUILTIN")
        check("community loads builtin session and replays its history", historyReplayed, builtinSessionId)
        check("community continues builtin session (end_turn)", stopReason === "end_turn", stopReason)
    } finally {
        await communityLoad.close()
    }

    // --- (1b) community 建立 → builtin 續聊 ---
    const communityNew = new AcpDriver(communityCommand, cwd)
    let communitySessionId = ""
    try {
        const { sessionId, stopReason } = await newSessionWithMarker(communityNew, cwd, "MARKER-FROM-COMMUNITY")
        communitySessionId = sessionId
        check("community new session prompt end_turn", stopReason === "end_turn", stopReason)
    } finally {
        await communityNew.close()
    }

    const builtinLoad = new AcpDriver(builtinCommand, cwd)
    try {
        const { historyReplayed, stopReason } = await loadAndContinue(builtinLoad, cwd, communitySessionId, "MARKER-FROM-COMMUNITY")
        check("builtin loads community session and replays its history", historyReplayed, communitySessionId)
        check("builtin continues community session (end_turn)", stopReason === "end_turn", stopReason)
    } finally {
        await builtinLoad.close()
    }

    // --- (2) V4 並存：同 cwd 各一 session 並行 prompt、事件分流無串音 ---
    const dualCwd = await mkdtemp(join(tmpdir(), "yuzora-dual-runtime-"))
    const dualBuiltin = new AcpDriver(builtinCommand, dualCwd)
    const dualCommunity = new AcpDriver(communityCommand, dualCwd)
    try {
        await Promise.all([
            dualBuiltin.request("initialize", { protocolVersion: 1, clientCapabilities: CLIENT_CAPS }, 120_000),
            dualCommunity.request("initialize", { protocolVersion: 1, clientCapabilities: CLIENT_CAPS }, 120_000)
        ])
        const [builtinCreated, communityCreated] = await Promise.all([
            dualBuiltin.request("session/new", { cwd: dualCwd, mcpServers: [] }, 180_000).then(asRecord),
            dualCommunity.request("session/new", { cwd: dualCwd, mcpServers: [] }, 180_000).then(asRecord)
        ])
        check(
            "dual runtimes create distinct sessions in the same cwd",
            builtinCreated.sessionId !== communityCreated.sessionId,
            `${String(builtinCreated.sessionId)} / ${String(communityCreated.sessionId)}`
        )
        const promptOf = (driver: AcpDriver, sessionId: unknown, marker: string) =>
            driver.request("session/prompt", {
                sessionId,
                prompt: [{ type: "text", text: `Reply with exactly the text ${marker} and stop.` }]
            }, 300_000).then(asRecord)
        const [builtinResult, communityResult] = await Promise.all([
            promptOf(dualBuiltin, builtinCreated.sessionId, "DUAL-BUILTIN-OK"),
            promptOf(dualCommunity, communityCreated.sessionId, "DUAL-COMMUNITY-OK")
        ])
        check("dual builtin prompt end_turn", builtinResult.stopReason === "end_turn", String(builtinResult.stopReason))
        check("dual community prompt end_turn", communityResult.stopReason === "end_turn", String(communityResult.stopReason))
        // 「各自有 marker」不驗——並行下模型後端可能 retry（實測 "Retrying
        // (attempt N/3)"），那是共用後端容量、非 runtime 干擾。隔離的實質＝
        // 對方的 marker 絕不得出現在自己的流（文字級無串音，確定性）。
        check(
            "turn text never crosses runtimes",
            !agentText(dualBuiltin).includes("DUAL-COMMUNITY-OK") && !agentText(dualCommunity).includes("DUAL-BUILTIN-OK")
        )
        const leaked = (driver: AcpDriver, foreignSessionId: unknown) => driver.lines.some((line) => {
            const params = asRecord(asRecord(line.msg).params)
            return line.dir === "a2c" && params.sessionId === foreignSessionId
        })
        check(
            "no cross-runtime event leakage",
            !leaked(dualBuiltin, communityCreated.sessionId) && !leaked(dualCommunity, builtinCreated.sessionId)
        )
    } finally {
        await dualBuiltin.close()
        await dualCommunity.close()
    }

    const failed = checks.filter((entry) => !entry.pass)
    console.log(failed.length === 0 ? `ALL ${checks.length} CHECKS PASSED` : `${failed.length} CHECKS FAILED`)
    process.exit(failed.length === 0 ? 0 : 1)
}

main().catch((error) => {
    console.error(error)
    process.exit(1)
})
