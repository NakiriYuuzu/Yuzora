import { writeFileSync } from "node:fs"
import { join } from "node:path"

const out = (name: string) => join("fixtures", "out", name)

const line = "const value = { alpha: 1, beta: 2, gamma: 3 } // padding padding padding\n"

function genSized(name: string, mb: number) {
    const target = mb * 1024 * 1024
    let buf = ""
    while (buf.length < target) buf += line
    writeFileSync(out(name), buf)
}

function genLongLine(name: string, chars: number) {
    writeFileSync(out(name), `const s = "${"x".repeat(chars)}"\n`)
}

genSized("f-1mb.ts", 1)
genSized("f-10mb.ts", 10)
genSized("f-30mb.ts", 30)
genSized("f-50mb.ts", 50)
genSized("f-80mb.ts", 80)
genLongLine("l-10k.ts", 10_000)
genLongLine("l-100k.ts", 100_000)
genLongLine("l-1m.ts", 1_000_000)
