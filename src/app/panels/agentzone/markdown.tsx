import { createElement, useEffect, useRef, useState, type CSSProperties, type ReactNode } from "react"
import { writeText } from "@tauri-apps/plugin-clipboard-manager"
import { openUrl } from "@tauri-apps/plugin-opener"
import { useTranslation } from "react-i18next"
import { CheckIcon, CopyIcon } from "lucide-react"

// AgentZone 專用的最小 markdown 渲染（spec P2 擴充；2026-07-21 使用者回饋再擴充）：
// fenced code、inline code/bold/italic/strike/link、heading（#–####）、巢狀無序/
// 有序清單（含 task checkbox）、blockquote、GFM table、hr。全部輸出 React 節點
// （無 dangerouslySetInnerHTML，天然免 XSS）；連結僅接受 http(s)，外開走
// @tauri-apps/plugin-opener（MarkdownPreview 同慣例）。圖片刻意不支援。
export function MinimalMarkdown({ text }: { text: string }) {
  return <>{renderMarkdownBlocks(text)}</>
}

function renderMarkdownBlocks(text: string): ReactNode[] {
  const nodes: ReactNode[] = []
  const fence = /```([^\n`]*)\n?([\s\S]*?)```/g
  let lastIndex = 0
  let match: RegExpExecArray | null

  while ((match = fence.exec(text)) !== null) {
    if (match.index > lastIndex) {
      nodes.push(...renderTextBlocks(text.slice(lastIndex, match.index), `b-${lastIndex}`))
    }
    const code = (match[2] ?? "").replace(/\n$/, "")
    nodes.push(<CodeBlock key={`pre-${match.index}`} code={code} />)
    lastIndex = fence.lastIndex
  }

  if (lastIndex < text.length) {
    nodes.push(...renderTextBlocks(text.slice(lastIndex), `b-${lastIndex}`))
  }

  return nodes.length > 0 ? nodes : [text]
}

// fenced code block＋右上角複製鈕（soak 回饋 2026-07-22）。剪貼簿走
// plugin-clipboard-manager（LogsSection 同慣例）；成功後短暫顯示勾勾。
function CodeBlock({ code }: { code: string }) {
  const { t } = useTranslation("panels")
  const [copied, setCopied] = useState(false)
  const resetTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  useEffect(() => () => clearTimeout(resetTimer.current), [])
  return (
    <div style={{ position: "relative" }} data-testid="md-code-block">
      <pre
        style={{
          margin: "8px 0 0",
          padding: "8px 10px",
          borderRadius: 8,
          background: "var(--yz-field)",
          border: "1px solid var(--line-1)",
          overflowX: "auto",
          fontFamily: "var(--font-mono)",
          fontSize: 11.5,
          lineHeight: 1.5,
        }}
      >
        <code>{code}</code>
      </pre>
      <button
        type="button"
        aria-label={t("agentZonePanel.copyCode")}
        title={copied ? t("agentZonePanel.copiedCode") : t("agentZonePanel.copyCode")}
        onClick={() => {
          void writeText(code).then(() => {
            setCopied(true)
            clearTimeout(resetTimer.current)
            resetTimer.current = setTimeout(() => setCopied(false), 1600)
          }).catch(() => undefined)
        }}
        style={{
          position: "absolute",
          top: 12,
          right: 6,
          display: "inline-flex",
          alignItems: "center",
          justifyContent: "center",
          width: 22,
          height: 22,
          padding: 0,
          borderRadius: 6,
          border: "1px solid var(--line-1)",
          background: "var(--paper-0)",
          color: copied ? "var(--yz-accent-ink, #4c7c22)" : "var(--ink-3)",
          cursor: "pointer",
          opacity: 0.85,
        }}
      >
        {copied ? <CheckIcon size={12} aria-hidden="true" /> : <CopyIcon size={12} aria-hidden="true" />}
      </button>
    </div>
  )
}

const HEADING_SIZE = ["1.25em", "1.15em", "1.05em", "1em"]
// [-*+] 或 1. / 1) 開頭（可縮排）＝清單項；縮排寬度決定巢狀層級。
const LIST_ITEM = /^(\s*)([-*+]|\d+[.)])\s+(.+)$/
// thematic break：整行只有同一種標記（-／*／_）三個以上，可夾空白。
const HR_LINE = /^ {0,3}([-*_])\s*(?:\1\s*){2,}$/

// 非 code 段落的行級解析：heading／hr／blockquote／table／清單收成語意區塊，
// 其餘行維持 pre-wrap 純文字（含 inline 解析），與原行為一致。
function renderTextBlocks(segment: string, keyPrefix: string): ReactNode[] {
  const nodes: ReactNode[] = []
  const lines = segment.split("\n")
  let plain: string[] = []
  let index = 0
  let key = 0

  const flushPlain = () => {
    const joined = plain.join("\n")
    plain = []
    if (joined.trim() === "") return
    nodes.push(
      <span key={`${keyPrefix}-txt-${key++}`} className="whitespace-pre-wrap">
        {renderInlineMarkdown(joined, `${keyPrefix}-in-${key}`)}
      </span>
    )
  }

  while (index < lines.length) {
    const line = lines[index]

    const heading = /^(#{1,4})\s+(.+)$/.exec(line)
    if (heading) {
      flushPlain()
      const level = heading[1].length
      nodes.push(
        createElement(
          `h${level + 2}`,
          {
            key: `${keyPrefix}-h-${key++}`,
            style: {
              margin: "0.6em 0 0.2em",
              fontSize: HEADING_SIZE[level - 1],
              fontWeight: 700,
              lineHeight: 1.35,
              color: "var(--ink-0)",
            },
          },
          renderInlineMarkdown(heading[2], `${keyPrefix}-hin-${key}`)
        )
      )
      index += 1
      continue
    }

    if (HR_LINE.test(line)) {
      flushPlain()
      nodes.push(
        <hr
          key={`${keyPrefix}-hr-${key++}`}
          style={{ border: 0, borderTop: "1px solid var(--line-2)", margin: "0.7em 0" }}
        />
      )
      index += 1
      continue
    }

    if (/^>\s?/.test(line)) {
      flushPlain()
      const quote: string[] = []
      while (index < lines.length && /^>\s?/.test(lines[index])) {
        quote.push(lines[index].replace(/^>\s?/, ""))
        index += 1
      }
      nodes.push(
        <blockquote
          key={`${keyPrefix}-q-${key++}`}
          className="whitespace-pre-wrap"
          style={{
            margin: "0.35em 0",
            padding: "2px 0 2px 10px",
            borderLeft: "2px solid var(--line-2)",
            color: "var(--ink-2)",
          }}
        >
          {renderInlineMarkdown(quote.join("\n"), `${keyPrefix}-qin-${key}`)}
        </blockquote>
      )
      continue
    }

    if (
      line.includes("|")
      && index + 1 < lines.length
      && lines[index + 1].includes("|")
      && isTableSeparator(lines[index + 1])
    ) {
      flushPlain()
      const header = splitTableRow(line)
      const aligns = splitTableRow(lines[index + 1]).map(cellAlign)
      index += 2
      const rows: string[][] = []
      while (index < lines.length && lines[index].includes("|") && lines[index].trim() !== "") {
        rows.push(splitTableRow(lines[index]))
        index += 1
      }
      nodes.push(renderTable(header, aligns, rows, `${keyPrefix}-tbl-${key++}`))
      continue
    }

    if (LIST_ITEM.test(line)) {
      flushPlain()
      const roots: ListNode[] = []
      const stack: { indent: number; node: ListNode }[] = []
      while (index < lines.length) {
        const item = LIST_ITEM.exec(lines[index])
        if (!item) break
        const indent = item[1].replace(/\t/g, "  ").length
        const node: ListNode = { text: item[3], ordered: /^\d/.test(item[2]), children: [] }
        while (stack.length > 0 && stack[stack.length - 1].indent >= indent) stack.pop()
        if (stack.length === 0) roots.push(node)
        else stack[stack.length - 1].node.children.push(node)
        stack.push({ indent, node })
        index += 1
      }
      nodes.push(renderList(roots, `${keyPrefix}-l-${key++}`))
      continue
    }

    plain.push(line)
    index += 1
  }

  flushPlain()
  return nodes
}

interface ListNode {
  text: string
  ordered: boolean
  children: ListNode[]
}

function renderList(items: ListNode[], keyPrefix: string): ReactNode {
  const ordered = items[0]?.ordered ?? false
  const style: CSSProperties = {
    margin: "0.35em 0",
    paddingLeft: "1.35em",
    display: "flex",
    flexDirection: "column",
    gap: 2,
    listStyle: ordered ? "decimal" : "disc",
  }
  const children = items.map((item, itemIndex) => {
    const task = /^\[( |x|X)\]\s+/.exec(item.text)
    const text = task ? item.text.slice(task[0].length) : item.text
    return (
      <li key={itemIndex} style={task ? { listStyle: "none", marginLeft: "-1.1em" } : undefined}>
        {task && <TaskBox checked={task[1] !== " "} />}
        {renderInlineMarkdown(text, `${keyPrefix}-${itemIndex}`)}
        {item.children.length > 0 && renderList(item.children, `${keyPrefix}-${itemIndex}c`)}
      </li>
    )
  })
  return createElement(ordered ? "ol" : "ul", { key: keyPrefix, style }, children)
}

function TaskBox({ checked }: { checked: boolean }) {
  return (
    <span
      aria-hidden="true"
      style={{
        display: "inline-flex",
        alignItems: "center",
        justifyContent: "center",
        width: 12,
        height: 12,
        borderRadius: 4,
        marginRight: 6,
        fontSize: 8,
        verticalAlign: "1px",
        border: `1.5px solid ${checked ? "var(--yz-accent)" : "var(--ink-4)"}`,
        background: checked ? "var(--yz-accent)" : "transparent",
        color: "#223005",
      }}
    >
      {checked ? "✓" : ""}
    </span>
  )
}

function isTableSeparator(line: string): boolean {
  if (!line.includes("-")) return false
  const cells = splitTableRow(line)
  return cells.length > 0 && cells.every((cell) => /^:?-+:?$/.test(cell))
}

function splitTableRow(line: string): string[] {
  let trimmed = line.trim()
  if (trimmed.startsWith("|")) trimmed = trimmed.slice(1)
  if (trimmed.endsWith("|")) trimmed = trimmed.slice(0, -1)
  return trimmed.split("|").map((cell) => cell.trim())
}

function cellAlign(separator: string): "center" | "right" | undefined {
  const left = separator.startsWith(":")
  const right = separator.endsWith(":")
  if (left && right) return "center"
  if (right) return "right"
  return undefined
}

function tableCellStyle(align: "center" | "right" | undefined, isHeader: boolean): CSSProperties {
  return {
    border: "1px solid var(--line-1)",
    padding: "3px 9px",
    textAlign: align,
    fontWeight: isHeader ? 600 : undefined,
    background: isHeader ? "var(--yz-field)" : undefined,
    color: isHeader ? "var(--ink-1)" : undefined,
  }
}

// 欄數以 header 為準（GFM 慣例）：多的儲存格捨棄、缺的補空。
function renderTable(
  header: string[],
  aligns: ("center" | "right" | undefined)[],
  rows: string[][],
  key: string
): ReactNode {
  return (
    <div key={key} style={{ overflowX: "auto", margin: "0.5em 0" }}>
      <table style={{ borderCollapse: "collapse", fontSize: "0.95em", lineHeight: 1.45 }}>
        <thead>
          <tr>
            {header.map((cell, column) => (
              <th key={column} style={tableCellStyle(aligns[column], true)}>
                {renderInlineMarkdown(cell, `${key}-h${column}`)}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, rowIndex) => (
            <tr key={rowIndex}>
              {header.map((_, column) => (
                <td key={column} style={tableCellStyle(aligns[column], false)}>
                  {renderInlineMarkdown(row[column] ?? "", `${key}-r${rowIndex}c${column}`)}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

const LINK_PATTERN = /^\[([^\]\n]+)\]\((https?:\/\/[^\s)]+)\)/

function renderInlineMarkdown(text: string, keyPrefix: string): ReactNode[] {
  const nodes: ReactNode[] = []
  let index = 0
  let key = 0

  while (index < text.length) {
    const codeIndex = text.indexOf("`", index)
    const starIndex = text.indexOf("*", index)
    const strikeIndex = text.indexOf("~~", index)
    const linkIndex = text.indexOf("[", index)
    const next = nextMarker(codeIndex, starIndex, strikeIndex, linkIndex)

    if (next === -1) {
      nodes.push(text.slice(index))
      break
    }

    if (next > index) nodes.push(text.slice(index, next))

    if (next === codeIndex) {
      const end = text.indexOf("`", codeIndex + 1)
      if (end === -1) {
        nodes.push(text.slice(codeIndex))
        break
      }
      nodes.push(
        <code
          key={`${keyPrefix}-code-${key++}`}
          style={{
            padding: "1px 4px",
            borderRadius: 5,
            background: "var(--yz-field)",
            border: "1px solid var(--line-1)",
            fontFamily: "var(--font-mono)",
            fontSize: "0.92em",
          }}
        >
          {text.slice(codeIndex + 1, end)}
        </code>
      )
      index = end + 1
      continue
    }

    if (next === linkIndex) {
      const link = LINK_PATTERN.exec(text.slice(linkIndex))
      if (!link) {
        nodes.push("[")
        index = linkIndex + 1
        continue
      }
      const [, label, href] = link
      nodes.push(
        <a
          key={`${keyPrefix}-a-${key++}`}
          href={href}
          rel="noreferrer noopener"
          style={{
            color: "var(--yz-accent-ink)",
            textDecoration: "underline",
            textUnderlineOffset: 2,
          }}
          onClick={(event) => {
            event.preventDefault()
            void openUrl(href).catch(() => {})
          }}
        >
          {label}
        </a>
      )
      index = linkIndex + link[0].length
      continue
    }

    if (next === strikeIndex && strikeIndex !== starIndex) {
      const end = text.indexOf("~~", strikeIndex + 2)
      if (end === -1 || end === strikeIndex + 2) {
        nodes.push(text.slice(strikeIndex, strikeIndex + 2))
        index = strikeIndex + 2
        continue
      }
      nodes.push(
        <s key={`${keyPrefix}-s-${key++}`}>{text.slice(strikeIndex + 2, end)}</s>
      )
      index = end + 2
      continue
    }

    if (text[starIndex + 1] === "*") {
      const end = text.indexOf("**", starIndex + 2)
      if (end === -1) {
        nodes.push(text.slice(starIndex))
        break
      }
      nodes.push(
        <strong key={`${keyPrefix}-strong-${key++}`}>{text.slice(starIndex + 2, end)}</strong>
      )
      index = end + 2
      continue
    }

    // 單一 *：斜體。開頭/結尾貼空白（如條列的「* 」殘留）視為字面星號。
    const italicEnd = text.indexOf("*", starIndex + 1)
    if (
      italicEnd === -1
      || italicEnd === starIndex + 1
      || text[starIndex + 1] === " "
      || text[italicEnd - 1] === " "
    ) {
      nodes.push("*")
      index = starIndex + 1
      continue
    }
    nodes.push(
      <em key={`${keyPrefix}-em-${key++}`}>{text.slice(starIndex + 1, italicEnd)}</em>
    )
    index = italicEnd + 1
  }

  return nodes
}

function nextMarker(...indexes: number[]): number {
  const present = indexes.filter((value) => value !== -1)
  return present.length > 0 ? Math.min(...present) : -1
}
