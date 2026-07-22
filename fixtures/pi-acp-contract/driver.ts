// P1 contract fixtures（.yuuzu/specs/pi-sdk-adapter-migration.html）——
// 最小 ACP client（raw stdio JSONL）＋全量 wire capture。刻意不用
// @agentclientprotocol/sdk：fixture 要記「線上真正跑的位元組」，中間不插任何
// SDK 行為；client 側請求參數逐字鏡射 yuzora acpConnection（initialize 的
// clientCapabilities、session/new 的 {cwd, mcpServers: []} 等）。
import { spawn, type ChildProcess } from "node:child_process"
import { readFile, writeFile } from "node:fs/promises"

export interface CapturedLine {
    seq: number
    /** c2a＝client→agent、a2c＝agent→client */
    dir: "c2a" | "a2c"
    /** ms since driver start */
    t: number
    msg: Record<string, unknown>
}

interface PendingRequest {
    method: string
    resolve: (value: unknown) => void
    reject: (error: Error) => void
}

interface NotificationWaiter {
    predicate: (msg: Record<string, unknown>) => boolean
    resolve: (msg: Record<string, unknown>) => void
}

export class AcpDriver {
    readonly lines: CapturedLine[] = []
    readonly stderrChunks: string[] = []
    /** elicitation/create 的自動回應（P3 probe 用）；未設定一律 cancel、回 undefined＝不回應。 */
    onElicitation?: (params: Record<string, unknown>) => Record<string, unknown> | undefined
    private child: ChildProcess
    private startedAt = Date.now()
    private seq = 0
    private nextId = 1
    private buffer = ""
    private pending = new Map<number, PendingRequest>()
    private waiters: NotificationWaiter[] = []
    private exited = false

    constructor(readonly command: string, readonly cwd: string) {
        // 經 shell 啟動，鏡射 yuzora（Rust 側同樣以 shell 解析 agent command）。
        this.child = spawn("/bin/sh", ["-c", command], {
            cwd,
            env: process.env,
            stdio: ["pipe", "pipe", "pipe"]
        })
        this.child.stdout?.setEncoding("utf8")
        this.child.stdout?.on("data", (chunk: string) => this.onStdout(chunk))
        this.child.stderr?.setEncoding("utf8")
        this.child.stderr?.on("data", (chunk: string) => {
            this.stderrChunks.push(chunk)
            if (this.stderrChunks.length > 200) this.stderrChunks.shift()
        })
        this.child.on("exit", () => {
            this.exited = true
            const error = new Error("agent process exited")
            for (const [, pendingRequest] of this.pending) pendingRequest.reject(error)
            this.pending.clear()
        })
    }

    private onStdout(chunk: string) {
        this.buffer += chunk
        const lines = this.buffer.split("\n")
        this.buffer = lines.pop() ?? ""
        for (const line of lines) {
            const trimmed = line.trim()
            if (trimmed === "") continue
            let msg: Record<string, unknown>
            try {
                msg = JSON.parse(trimmed) as Record<string, unknown>
            } catch {
                this.stderrChunks.push(`[driver] non-JSON stdout line: ${trimmed.slice(0, 200)}`)
                continue
            }
            this.capture("a2c", msg)
            this.dispatch(msg)
        }
    }

    private capture(dir: "c2a" | "a2c", msg: Record<string, unknown>) {
        this.seq += 1
        this.lines.push({ seq: this.seq, dir, t: Date.now() - this.startedAt, msg })
    }

    private dispatch(msg: Record<string, unknown>) {
        const hasId = msg.id !== undefined && msg.id !== null
        const isResponse = hasId && msg.method === undefined
        if (isResponse) {
            const pendingRequest = this.pending.get(msg.id as number)
            if (!pendingRequest) return
            this.pending.delete(msg.id as number)
            if (msg.error !== undefined) {
                const error = msg.error as { code?: number; message?: string }
                pendingRequest.reject(
                    new Error(`${pendingRequest.method} failed: [${error.code}] ${error.message}`)
                )
            } else {
                pendingRequest.resolve(msg.result)
            }
            return
        }
        if (hasId && typeof msg.method === "string") {
            void this.answerAgentRequest(msg.id as number | string, msg.method, msg.params)
            return
        }
        // notification
        const remaining: NotificationWaiter[] = []
        for (const waiter of this.waiters) {
            if (waiter.predicate(msg)) waiter.resolve(msg)
            else remaining.push(waiter)
        }
        this.waiters = remaining
    }

    // agent→client 請求的自動回應：permission 選第一個 option、fs 讀寫走真檔案
    //（yuzora 亦如此）、terminal 回 method not found（pi-acp 不用 client terminal，
    // 若未來出現會如實錄進 fixture 供比對）。
    private async answerAgentRequest(id: number | string, method: string, params: unknown) {
        const record = (params ?? {}) as Record<string, unknown>
        try {
            if (method === "session/request_permission") {
                const options = Array.isArray(record.options) ? record.options as { optionId: string }[] : []
                const optionId = options[0]?.optionId
                this.respond(id, optionId
                    ? { outcome: { outcome: "selected", optionId } }
                    : { outcome: { outcome: "cancelled" } })
                return
            }
            if (method === "fs/read_text_file") {
                const content = await readFile(String(record.path), "utf8")
                this.respond(id, { content })
                return
            }
            if (method === "fs/write_text_file") {
                await writeFile(String(record.path), String(record.content ?? ""), "utf8")
                this.respond(id, {})
                return
            }
            if (method === "elicitation/create") {
                if (!this.onElicitation) {
                    this.respond(id, { action: "cancel" })
                    return
                }
                const answer = this.onElicitation(record)
                // undefined＝故意不回應（讓 adapter 端 timeout 路徑可測）。
                if (answer !== undefined) this.respond(id, answer)
                return
            }
            this.respondError(id, -32601, `recorder does not implement ${method}`)
        } catch (error) {
            this.respondError(id, -32603, error instanceof Error ? error.message : String(error))
        }
    }

    private send(msg: Record<string, unknown>) {
        if (this.exited) throw new Error("agent process already exited")
        this.capture("c2a", msg)
        this.child.stdin?.write(`${JSON.stringify(msg)}\n`)
    }

    private respond(id: number | string, result: unknown) {
        this.send({ jsonrpc: "2.0", id, result })
    }

    private respondError(id: number | string, code: number, message: string) {
        this.send({ jsonrpc: "2.0", id, error: { code, message } })
    }

    request(method: string, params: unknown, timeoutMs = 30_000): Promise<unknown> {
        const id = this.nextId++
        const promise = new Promise<unknown>((resolve, reject) => {
            this.pending.set(id, { method, resolve, reject })
            setTimeout(() => {
                if (!this.pending.has(id)) return
                this.pending.delete(id)
                reject(new Error(`${method} timed out after ${timeoutMs}ms`))
            }, timeoutMs)
        })
        this.send({ jsonrpc: "2.0", id, method, params })
        return promise
    }

    notify(method: string, params: unknown) {
        this.send({ jsonrpc: "2.0", method, params })
    }

    /** 等到任一 a2c notification 符合條件（先掃已捕獲的，避免 race）。 */
    waitForNotification(
        predicate: (msg: Record<string, unknown>) => boolean,
        timeoutMs = 60_000
    ): Promise<Record<string, unknown>> {
        const seen = this.lines.find(
            (line) => line.dir === "a2c" && line.msg.method !== undefined && line.msg.id === undefined && predicate(line.msg)
        )
        if (seen) return Promise.resolve(seen.msg)
        return new Promise((resolve, reject) => {
            const waiter: NotificationWaiter = { predicate, resolve }
            this.waiters.push(waiter)
            setTimeout(() => {
                this.waiters = this.waiters.filter((candidate) => candidate !== waiter)
                reject(new Error(`waitForNotification timed out after ${timeoutMs}ms`))
            }, timeoutMs)
        })
    }

    async close(graceMs = 300) {
        await new Promise((resolve) => setTimeout(resolve, graceMs))
        this.child.stdin?.end()
        await new Promise((resolve) => setTimeout(resolve, 200))
        if (!this.exited) this.child.kill()
    }

    stderrTail(): string {
        return this.stderrChunks.join("").split("\n").slice(-12).join("\n")
    }
}
