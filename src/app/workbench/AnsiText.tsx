import { useTranslation } from "react-i18next"

import { parseAnsiSegments } from "./ansiSegments"

export function AnsiText({ text }: { text: string }) {
  const { t } = useTranslation("workbench")
  const { segments, truncated, tooLarge } = parseAnsiSegments(text)
  return (
    <>
      {segments.map((segment, index) => (
        <span key={`ansi-${index}`} style={segment.style}>
          {segment.text}
        </span>
      ))}
      {tooLarge ? (
        <span role="status">{t("ansiText.tooLarge")}</span>
      ) : truncated ? (
        <span role="status">{t("ansiText.truncated")}</span>
      ) : null}
    </>
  )
}
