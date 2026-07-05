import { revealItemInDir, openPath } from "@tauri-apps/plugin-opener"
import { writeText } from "@tauri-apps/plugin-clipboard-manager"
import type { OpenFileResult } from "../lib/types"

function formatSize(bytes: number): string {
    return `${(bytes / 1024 / 1024).toFixed(1)} MB`
}

export function SpecialFileView({
    path,
    result
}: {
    path: string
    result: Extract<OpenFileResult, { kind: "tooLarge" | "binary" }>
}) {
    return (
        <div className="special-file">
            <p>
                {result.kind === "tooLarge"
                    ? `檔案過大（${formatSize(result.size)}），不載入編輯器`
                    : "二進位檔案，無法以文字編輯器開啟"}
            </p>
            <div className="special-actions">
                <button onClick={() => void revealItemInDir(path)}>在系統中顯示</button>
                <button onClick={() => void openPath(path)}>以外部程式開啟</button>
                <button onClick={() => void writeText(path)}>複製路徑</button>
            </div>
        </div>
    )
}
