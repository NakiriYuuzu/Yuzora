import { useEffect, useRef } from "react"
import { useTranslation } from "react-i18next"

import type { TranscriptEntry } from "@/agent/acpTypes"
import type { SessionState } from "@/state/agentStore"

import { ActivityChain } from "./ActivityChain"
import { PlanBlock, TranscriptBlock } from "./blocks"
import { MinimalMarkdown } from "./markdown"
import { segmentTranscript } from "./transcriptSegments"

export type AgentVisual = {
  label: string
  short: string
  color: string
  bg: string
}

export function TranscriptList({
  sessionId,
  session,
  agent,
}: {
  sessionId: string
  session: SessionState
  agent: AgentVisual
}) {
  const { t } = useTranslation("panels")
  const entries = dedupeInfoBanner(session.transcript, session.infoBanner)
  const segments = segmentTranscript(entries)
  const tail = entries.at(-1)
  const cursorOnTail =
    session.tone === "run"
    && tail !== undefined
    && "who" in tail
    && tail.who === "agent"
    && tail.streaming
  const scrollRef = useRef<HTMLDivElement>(null)
  const nearBottomRef = useRef(true)
  function handleScroll() {
    const el = scrollRef.current
    if (!el) return
    nearBottomRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80
  }
  useEffect(() => {
    const el = scrollRef.current
    if (!el || !nearBottomRef.current) return
    el.scrollTop = el.scrollHeight
  }, [session.transcript])
  return (
    <div
      ref={scrollRef}
      data-testid="agent-transcript"
      onScroll={handleScroll}
      className="yzs"
      style={{
        flex: 1,
        minHeight: 0,
        overflow: "auto",
        padding: "12px 14px",
        display: "flex",
        flexDirection: "column",
        gap: 12,
        // Atelier（P2）：畫布交給外層的 ambient 漸層（ActiveAgentSession）。
        background: "transparent",
      }}
    >
      {entries.length === 0 ? (
        <div className="flex min-h-full items-center justify-center text-[12.5px] text-(--ink-3)">
          {t("agentZonePanel.noTranscript")}
        </div>
      ) : (
        segments.map((segment) =>
          segment.type === "activity" ? (
            <ActivityChain
              key={segment.id}
              entries={segment.entries}
              live={session.tone === "run" && segment.entries.at(-1) === tail}
            />
          ) : (
            <TranscriptEntryRow
              key={segment.entry.id}
              entry={segment.entry}
              sessionId={sessionId}
              agent={agent}
              showStreamingCursor={cursorOnTail && segment.entry === tail}
            />
          )
        )
      )}
    </div>
  )
}

function dedupeInfoBanner(transcript: TranscriptEntry[], infoBanner?: string | null): TranscriptEntry[] {
  if (!infoBanner) return transcript
  const first = transcript[0]
  if (first && "who" in first && first.who === "agent" && first.text.trim() === infoBanner.trim()) {
    return transcript.slice(1)
  }
  return transcript
}

function TranscriptEntryRow({
  entry,
  sessionId,
  agent,
  showStreamingCursor,
}: {
  entry: TranscriptEntry
  sessionId: string
  agent: AgentVisual
  showStreamingCursor: boolean
}) {
  const { t } = useTranslation("panels")
  if ("who" in entry) {
    const you = entry.who === "you"
    // 純圖片訊息的 text 只剩 "[image]" 佔位（store 以它派生 session 標題）；
    // 縮圖列已呈現內容，佔位字不再重複顯示。混合訊息保留原文中的佔位。
    const placeholderOnlyText =
      you &&
      entry.images !== undefined &&
      entry.images.length > 0 &&
      /^(\s*\[image\])+\s*$/.test(entry.text)
    // Atelier（P2）：user＝右側玻璃氣泡；agent＝頭像＋全寬內容流（無氣泡，
    // 拿回 94% maxWidth 的空間浪費）。sender 標籤列移除、改 aria-label。
    if (you) {
      return (
        <div style={{ display: "flex", justifyContent: "flex-end" }}>
          <div
            data-testid="agent-message-bubble"
            data-sender="you"
            aria-label={t("agentZonePanel.you")}
            style={{
              maxWidth: "76%",
              padding: "9px 13px",
              borderRadius: 16,
              borderBottomRightRadius: 5,
              fontSize: 12.5,
              lineHeight: 1.55,
              whiteSpace: "pre-wrap",
              background: "var(--yz-active)",
              color: "var(--ink-0)",
              border: "1px solid var(--line-1)",
              backdropFilter: "blur(10px)",
              WebkitBackdropFilter: "blur(10px)",
              boxShadow: "var(--shadow-xs)",
            }}
          >
            {entry.images && entry.images.length > 0 && (
              <div
                data-testid="message-image-strip"
                style={{
                  display: "flex",
                  flexWrap: "wrap",
                  gap: 6,
                  marginBottom: entry.text && !placeholderOnlyText ? 8 : 0,
                }}
              >
                {entry.images.map((image, index) => (
                  <img
                    key={index}
                    src={image.dataUrl}
                    alt=""
                    style={{
                      maxWidth: 180,
                      maxHeight: 120,
                      borderRadius: 8,
                      border: "1px solid var(--line-1)",
                      objectFit: "cover",
                    }}
                  />
                ))}
              </div>
            )}
            {placeholderOnlyText ? null : entry.text}
          </div>
        </div>
      )
    }
    return (
      <div style={{ display: "flex", gap: 9, alignItems: "flex-start" }}>
        <span
          aria-hidden="true"
          style={{
            width: 20,
            height: 20,
            borderRadius: "50%",
            flex: "0 0 auto",
            marginTop: 2,
            display: "inline-flex",
            alignItems: "center",
            justifyContent: "center",
            fontSize: 10.5,
            fontWeight: 700,
            background: agent.bg,
            color: agent.color,
          }}
        >
          {agent.short}
        </span>
        <div
          data-testid="agent-message-bubble"
          data-sender="agent"
          aria-label={agent.label}
          style={{
            flex: 1,
            minWidth: 0,
            fontSize: 13,
            lineHeight: 1.62,
            color: "var(--ink-1)",
            whiteSpace: "pre-wrap",
          }}
        >
          <MinimalMarkdown text={entry.text} />
          {showStreamingCursor && <StreamingCursor />}
        </div>
      </div>
    )
  }

  // tool／thought 已由 segmentTranscript 聚合進 ActivityChain，不會到達這裡；
  // plan 走 checkbox frost 卡，其餘 block（diff／perm／error／notice）維持
  // TranscriptBlock（P2 玻璃化）。
  if (entry.kind === "plan") return <PlanBlock entry={entry} />
  return <TranscriptBlock entry={entry} sessionId={sessionId} />
}

function StreamingCursor() {
  // Atelier（P2）：lime 呼吸點取代方塊游標；reduced-motion 停用動畫（styles.css）。
  return (
    <span
      data-testid="agent-streaming-cursor"
      aria-hidden="true"
      style={{
        display: "inline-block",
        width: 7,
        height: 7,
        marginLeft: 5,
        verticalAlign: 0,
        background: "var(--yz-accent)",
        borderRadius: "50%",
        animation: "yzpulse 1.4s ease-in-out infinite",
      }}
    />
  )
}
