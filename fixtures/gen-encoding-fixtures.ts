import { writeFileSync } from "node:fs"
import { join } from "node:path"

const out = (name: string) => join("fixtures", "out", name)

function genUtf16Le(name: string, text: string) {
    // FF FE BOM + text 的 UTF-16LE 位元組（Buffer "utf16le" 不含 BOM，手動前綴）
    const bom = Buffer.from([0xff, 0xfe])
    const body = Buffer.from(text, "utf16le")
    writeFileSync(out(name), Buffer.concat([bom, body]))
}

function genLegacy(name: string) {
    // EF BB BF（UTF-8 BOM）+ C3 28（不合法 UTF-8 continuation byte）+ "AB"（ASCII 尾）
    // bytes 與 fs_service.rs 的 open_file_non_utf8_falls_back_windows_1252 測試逐位元組一致
    const bytes = Buffer.from([0xef, 0xbb, 0xbf, 0xc3, 0x28, 0x41, 0x42])
    writeFileSync(out(name), bytes)
}

genUtf16Le("enc-utf16le.txt", "hello 世界")
genLegacy("enc-legacy.txt")
