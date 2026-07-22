// 驗證錄音健全性：bun fixtures/pi-acp-contract/check.ts recordings/*.jsonl
import { readFile } from "node:fs/promises"

import { invariants, parseRecording, collapse } from "./contract"

async function main() {
    const paths = process.argv.slice(2)
    if (paths.length === 0) {
        console.error("usage: bun fixtures/pi-acp-contract/check.ts <recording.jsonl...>")
        process.exit(2)
    }
    let failed = false
    for (const path of paths) {
        const recording = parseRecording(await readFile(path, "utf8"))
        const findings = invariants(recording)
        const errors = findings.filter((finding) => finding.level === "error")
        const events = collapse(recording)
        console.log(`${path}: ${recording.lines.length} wire messages → ${events.length} collapsed events`)
        for (const finding of findings) console.log(`  [${finding.level}] ${finding.message}`)
        if (errors.length > 0) failed = true
        else console.log("  OK")
    }
    if (failed) process.exit(1)
}

main().catch((error) => {
    console.error(error)
    process.exit(1)
})
