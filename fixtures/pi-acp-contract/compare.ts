// 兩份錄音的結構比對（P2 parity gate 用）：
//   bun fixtures/pi-acp-contract/compare.ts baseline.jsonl candidate.jsonl [--dump]
// --dump：另外印出兩邊完整 collapsed 簽名序列（除錯用）。
import { readFile } from "node:fs/promises"

import { collapse, compare, parseRecording } from "./contract"

async function main() {
    const args = process.argv.slice(2)
    const dump = args.includes("--dump")
    const paths = args.filter((arg) => arg !== "--dump")
    if (paths.length !== 2) {
        console.error("usage: bun fixtures/pi-acp-contract/compare.ts <a.jsonl> <b.jsonl> [--dump]")
        process.exit(2)
    }
    const a = parseRecording(await readFile(paths[0], "utf8"))
    const b = parseRecording(await readFile(paths[1], "utf8"))
    if (dump) {
        for (const [label, recording] of [["A", a], ["B", b]] as const) {
            console.log(`--- ${label}: ${label === "A" ? paths[0] : paths[1]}`)
            for (const event of collapse(recording)) {
                console.log(`  ${event.sig}${event.count > 1 ? ` ×${event.count}` : ""}`)
            }
        }
    }
    const result = compare(a, b)
    for (const line of result.report) console.log(line)
    process.exit(result.equal ? 0 : 1)
}

main().catch((error) => {
    console.error(error)
    process.exit(1)
})
