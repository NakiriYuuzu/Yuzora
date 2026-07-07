import { revealItemInDir, openPath } from "@tauri-apps/plugin-opener"
import { writeText } from "@tauri-apps/plugin-clipboard-manager"
import { useTranslation } from "react-i18next"
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
    const { t } = useTranslation("panels")
    return (
        <div className="special-file">
            <p>
                {result.kind === "tooLarge"
                    ? t("specialFileView.tooLarge", { size: formatSize(result.size) })
                    : t("specialFileView.binary")}
            </p>
            <div className="special-actions">
                <button onClick={() => void revealItemInDir(path)}>{t("specialFileView.revealInSystem")}</button>
                <button onClick={() => void openPath(path)}>{t("specialFileView.openExternally")}</button>
                <button onClick={() => void writeText(path)}>{t("specialFileView.copyPath")}</button>
            </div>
        </div>
    )
}
