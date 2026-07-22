// P3 gate 驗證：form elicitation 全鏈路（pi extension ctx.ui.select/confirm/input
// → session host requestElicit → parent elicitation/create → client accept →
// 值回到 extension tool）。以 project-local extension（<cwd>/.pi/extensions）提供
// 決定性觸發源。用法：
//   PI_ACP_PI_COMMAND=... bun fixtures/pi-acp-contract/elicitation-probe.ts --command "node adapters/yuzora-pi-acp/dist/index.mjs"
import { mkdtemp, mkdir, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"

import { AcpDriver } from "./driver"

function asRecord(value: unknown): Record<string, unknown> {
    return value && typeof value === "object" ? value as Record<string, unknown> : {}
}

const PROBE_EXTENSION = `
export default function probe(pi) {
    pi.registerTool({
        name: "pick_color",
        label: "Pick color",
        description: "Ask the user to pick a color, confirm it, and collect notes.",
        parameters: { type: "object", properties: {}, required: [] },
        async execute(_toolCallId, _params, _signal, _onUpdate, ctx) {
            const value = await ctx.ui.select("Pick a color", ["red", "blue"])
            const confirmed = await ctx.ui.confirm("Confirm choice", "Use this color?")
            const note = await ctx.ui.input("Add a note", "type something")
            const long = await ctx.ui.editor("Long note", "prefill-text")
            const declined = await ctx.ui.select("Decline me", ["x"])
            const timedOut = await ctx.ui.select("Slow one", ["x"], { timeout: 1200 })
            const text = "picked=" + (value ?? "none") + " confirmed=" + confirmed
                + " note=" + (note ?? "none") + " long=" + (long ?? "none")
                + " declined=" + (declined ?? "none") + " timedOut=" + (timedOut ?? "none")
            return { content: [{ type: "text", text }], details: {} }
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
    const cwd = await mkdtemp(join(tmpdir(), "yuzora-pi-elicit-"))
    await mkdir(join(cwd, ".pi", "extensions"), { recursive: true })
    await writeFile(join(cwd, ".pi", "extensions", "yuzora-elicit-probe.ts"), PROBE_EXTENSION, "utf8")

    const driver = new AcpDriver(command, cwd)
    const elicitations: Record<string, unknown>[] = []
    driver.onElicitation = (params) => {
        elicitations.push(params)
        const message = String(params.message ?? "")
        if (message === "Decline me") return { action: "decline" }
        if (message === "Slow one") return undefined // 不回應 → adapter 端 timeout 路徑
        const properties = asRecord(asRecord(params.requestedSchema).properties)
        if (properties.choice) return { action: "accept", content: { choice: "red" } }
        if (properties.confirmed) return { action: "accept", content: { confirmed: true } }
        if (properties.value) {
            const multiline = asRecord(asRecord(params._meta).yuzora).multiline === true
            return { action: "accept", content: { value: multiline ? "edited-multiline" : "hello-from-yuzora" } }
        }
        return { action: "cancel" }
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
        const result = asRecord(await driver.request("session/prompt", {
            sessionId: created.sessionId,
            prompt: [{
                type: "text",
                text: "Call the pick_color tool exactly once. After it returns, reply with only the text that the tool returned."
            }]
        }, 300_000))

        check("prompt ended normally", result.stopReason === "end_turn", String(result.stopReason))
        check("six elicitations received", elicitations.length === 6, String(elicitations.length))
        const modes = elicitations.map((entry) => String(entry.mode))
        check("all elicitations are form mode", modes.every((mode) => mode === "form"), modes.join(","))
        const messages = elicitations.map((entry) => String(entry.message))
        check(
            "select/confirm/input/editor/decline/timeout arrived in order",
            messages.join("|") === "Pick a color|Confirm choice|Add a note|Long note|Decline me|Slow one",
            messages.join(" | ")
        )
        const editorRequest = elicitations[3]
        check(
            "editor request carries multiline _meta and prefill default",
            asRecord(asRecord(asRecord(editorRequest)._meta).yuzora).multiline === true
                && JSON.stringify(editorRequest).includes("prefill-text"),
            JSON.stringify(asRecord(editorRequest)._meta)
        )
        const transcriptText = driver.lines
            .map((line) => asRecord(asRecord(asRecord(line.msg).params).update))
            .filter((update) => update.sessionUpdate === "tool_call_update" || update.sessionUpdate === "agent_message_chunk")
            .map((update) => JSON.stringify(update))
            .join("\n")
        check(
            "tool observed accept/decline/timeout outcomes",
            transcriptText.includes("picked=red") && transcriptText.includes("confirmed=true")
                && transcriptText.includes("note=hello-from-yuzora") && transcriptText.includes("long=edited-multiline")
                && transcriptText.includes("declined=none") && transcriptText.includes("timedOut=none"),
            transcriptText.includes("picked=") ? "values found" : "no tool output captured"
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
