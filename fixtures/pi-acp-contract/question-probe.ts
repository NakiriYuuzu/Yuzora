// P4 gate 驗證：pi-questions question tool 的真答題（spec V3）＋未知 custom
// fail-fast（spec V5）。question 走 host 的 event bridge：pi-questions service
// （Symbol.for registry）opened 事件 → 多題 form elicitation → reply/reject。
// 需要使用者環境已安裝 @vanillagreen/pi-questions（pi settings packages）。
// 注意：不要設 PI_ACP_PI_COMMAND wrapper——question tool 必須存在（builtin
// adapter 本來就不讀該 env）。用法：
//   bun fixtures/pi-acp-contract/question-probe.ts --command "node adapters/yuzora-pi-acp/dist/index.mjs"
import { mkdtemp, mkdir, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"

import { AcpDriver } from "./driver"

function asRecord(value: unknown): Record<string, unknown> {
    return value && typeof value === "object" ? value as Record<string, unknown> : {}
}

// 未知 custom 的 fail-fast 觸發源（V5）：沒有 question 在途 → custom() 即時
// throw → tool 收 error 收斂，turn 正常結束、永不 hang。
const BROKEN_CUSTOM_EXTENSION = `
export default function brokenCustom(pi) {
    pi.registerTool({
        name: "broken_ui",
        label: "Broken UI",
        description: "Opens a custom TUI component (unsupported over ACP).",
        parameters: { type: "object", properties: {}, required: [] },
        async execute(_toolCallId, _params, _signal, _onUpdate, ctx) {
            try {
                const result = await ctx.ui.custom(() => ({ invalidate() {}, render() { return [] } }))
                return { content: [{ type: "text", text: "custom resolved: " + JSON.stringify(result) }], details: {} }
            } catch (error) {
                return { content: [{ type: "text", text: "custom rejected: " + String(error && error.message || error) }], details: {} }
            }
        }
    })
}
`

const checks: { name: string; pass: boolean; detail?: string }[] = []
function check(name: string, pass: boolean, detail?: string) {
    checks.push({ name, pass, ...(detail ? { detail } : {}) })
    console.log(`${pass ? "PASS" : "FAIL"} ${name}${detail ? ` — ${detail}` : ""}`)
}

async function main() {
    const commandFlagIndex = process.argv.indexOf("--command")
    const command = commandFlagIndex !== -1
        ? process.argv[commandFlagIndex + 1]
        : "node adapters/yuzora-pi-acp/dist/index.mjs"
    const cwd = await mkdtemp(join(tmpdir(), "yuzora-pi-question-"))
    await mkdir(join(cwd, ".pi", "extensions"), { recursive: true })
    await writeFile(join(cwd, ".pi", "extensions", "yuzora-broken-custom.ts"), BROKEN_CUSTOM_EXTENSION, "utf8")

    const driver = new AcpDriver(command, cwd)
    const elicitations: Record<string, unknown>[] = []
    // 第一發 accept（多題：單選＋多選＋custom 文字）、第二發 decline。答案值
    // 全取自 schema 本身（不假設模型照抄我們指定的選項字面）。
    let accepted: { single: string; multi: string[]; custom: string } | undefined
    driver.onElicitation = (params) => {
        elicitations.push(params)
        const properties = asRecord(asRecord(params.requestedSchema).properties)
        if (elicitations.length > 1) return { action: "decline" }
        const entries = Object.entries(properties).map(([key, raw]) => [key, asRecord(raw)] as const)
        const single = entries.find(([, prop]) => prop.type === "string" && Array.isArray(prop.oneOf))
        const multi = entries.find(([, prop]) => prop.type === "array")
        const customKey = entries.find(([key, prop]) => prop.type === "string" && !prop.oneOf && key.endsWith("custom"))
        if (!single || !multi) return { action: "cancel" }
        const singleChoice = String(asRecord((single[1].oneOf as unknown[])[0]).const ?? "")
        const anyOf = asRecord(multi[1].items).anyOf as unknown[]
        const multiChoices = anyOf.slice(0, 2).map((item) => String(asRecord(item).const ?? ""))
        accepted = { single: singleChoice, multi: multiChoices, custom: "turquoise-custom-answer" }
        return {
            action: "accept",
            content: {
                [single[0]]: singleChoice,
                [multi[0]]: multiChoices,
                ...(customKey ? { [customKey[0]]: "" } : {}),
                [`${multi[0]}custom`]: accepted.custom
            }
        }
    }
    try {
        await driver.request("initialize", {
            protocolVersion: 1,
            clientCapabilities: {
                fs: { readTextFile: true, writeTextFile: true },
                terminal: true,
                session: { configOptions: { boolean: {} } },
                elicitation: { form: {} }
            }
        }, 60_000)
        const created = asRecord(await driver.request("session/new", { cwd, mcpServers: [] }, 120_000))
        const sessionId = created.sessionId

        // --- V3：question 可答，答案回到 tool ---
        const answered = asRecord(await driver.request("session/prompt", {
            sessionId,
            prompt: [{
                type: "text",
                text: "Call the question tool exactly once with two questions: "
                    + "(1) header 'Fruit' question 'Pick one fruit.' with options Apple, Banana, Cherry and multiple false; "
                    + "(2) header 'Colors' question 'Pick colors.' with options Red, Green, Blue and multiple set to true. "
                    + "After the tool returns, reply with the exact answers array the tool returned, then stop."
            }]
        }, 300_000))
        check("question prompt ended normally", answered.stopReason === "end_turn", String(answered.stopReason))
        check("exactly one elicitation for the multi-question form", elicitations.length === 1, String(elicitations.length))
        const schema = asRecord(elicitations[0]?.requestedSchema)
        const schemaProperties = asRecord(schema.properties)
        const propShapes = Object.entries(schemaProperties)
            .map(([key, raw]) => `${key}:${String(asRecord(raw).type)}`)
            .sort()
            .join(",")
        check(
            "schema has single-select, multiselect and custom text fields",
            Object.values(schemaProperties).some((raw) => Array.isArray(asRecord(raw).oneOf))
                && Object.values(schemaProperties).some((raw) => asRecord(raw).type === "array")
                && Object.keys(schemaProperties).some((key) => key.endsWith("custom")),
            propShapes
        )
        // 答案值也出現在 schema 與 tool rawInput——只認含 "answers" 鍵的 a2c
        // update（question tool result＝JSON.stringify({requestId, answers})，
        // rawInput／schema 都沒有這個鍵），避免斷言空洞。
        const answersText = driver.lines
            .filter((line) => line.dir === "a2c")
            .map((line) => JSON.stringify(asRecord(asRecord(line.msg).params).update ?? {}))
            .filter((text) => text.includes("\\\"answers\\\"") || text.includes("\"answers\""))
            .join("\n")
        const answeredValuesVisible = accepted !== undefined
            && answersText.includes(accepted.single)
            && accepted.multi.every((value) => answersText.includes(value))
            && answersText.includes(accepted.custom)
        check(
            "accepted answers (single + multi + custom) reached the tool result",
            answeredValuesVisible,
            accepted ? `single=${accepted.single} multi=${accepted.multi.join("/")} custom=${accepted.custom}` : "no accept sent"
        )

        // --- decline：question 收斂 cancelled、turn 正常結束 ---
        const declined = asRecord(await driver.request("session/prompt", {
            sessionId,
            prompt: [{
                type: "text",
                text: "Call the question tool once with one question (header 'Retry' question 'Go on?' options Yes, No). "
                    + "If it is cancelled or declined, do not ask again; reply with the single word DECLINED-OK."
            }]
        }, 300_000))
        check("declined question still ends the turn", declined.stopReason === "end_turn", String(declined.stopReason))
        check("second elicitation was sent and declined", elicitations.length === 2, String(elicitations.length))

        // --- V5：未知 custom fail-fast，永不 hang ---
        const linesBefore = driver.lines.length
        const broken = asRecord(await driver.request("session/prompt", {
            sessionId,
            prompt: [{
                type: "text",
                text: "Call the broken_ui tool exactly once. Reply with the exact text it returned, then stop."
            }]
        }, 180_000))
        const brokenWire = driver.lines.slice(linesBefore).map((line) => JSON.stringify(line.msg)).join("\n")
        check("unknown custom turn ends normally (no hang)", broken.stopReason === "end_turn", String(broken.stopReason))
        check(
            "unknown custom rejects fail-fast with the unsupported error",
            brokenWire.includes("custom rejected") && brokenWire.includes("not supported over ACP"),
            brokenWire.includes("custom rejected") ? "rejection surfaced" : "no rejection text found"
        )
    } finally {
        await driver.close()
    }
    const failed = checks.filter((entry) => !entry.pass)
    console.log(failed.length === 0 ? `ALL ${checks.length} CHECKS PASSED` : `${failed.length} CHECKS FAILED`)
    process.exit(failed.length === 0 ? 0 : 1)
}

main().catch((error) => {
    console.error(error)
    process.exit(1)
})
