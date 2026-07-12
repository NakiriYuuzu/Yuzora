import React from "react";
import {
  Easing,
  interpolate,
  useCurrentFrame,
  useVideoConfig,
} from "remotion";
import { fonts, t, EASE } from "./theme";
import { Cursor, PulseDot, typed, typedDone, useRise } from "./ui";
import { SideRow, Workbench } from "./Workbench";

type Lang = "zh" | "en";

const CLAUDE = { color: "#c0562f", bg: "rgba(192,86,47,0.12)" };
const RUN = { dot: "#f5820a", bg: "#ffe9d2", fg: "#b45309" };

const COPY: Record<Lang, Record<string, string>> = {
  zh: {
    session: "handler 錯誤處理",
    user: "幫我看這個 handler 的錯誤處理",
    reply: "這裡的 query 錯誤被吞掉了——建議回傳 Result，在 route 層統一處理。",
    toolRead: "讀取 src/routes.rs",
    running: "執行中…",
    done: "完成",
    perm: "允許修改 src/db.rs？",
    allow: "允許",
    deny: "拒絕",
    check: "cargo check",
    passed: "通過",
    tone: "執行中",
    composer: "傳訊息給 Agent…",
    newSession: "新增 session",
    you: "YOU",
  },
  en: {
    session: "handler error review",
    user: "Review the error handling in this handler",
    reply: "The query error is swallowed here — return a Result and handle it once at the route layer.",
    toolRead: "Read src/routes.rs",
    running: "running…",
    done: "done",
    perm: "Allow edit to src/db.rs?",
    allow: "Allow",
    deny: "Deny",
    check: "cargo check",
    passed: "passed",
    tone: "running",
    composer: "Message the agent…",
    newSession: "New session",
    you: "YOU",
  },
};

/* 對話氣泡（AgentZonePanel L810-860 spec） */
const Bubble: React.FC<{
  from: number;
  user?: boolean;
  label?: string;
  children: React.ReactNode;
}> = ({ from, user = false, label, children }) => {
  const rise = useRise(from);
  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        alignItems: user ? "flex-end" : "flex-start",
        gap: 4,
        opacity: rise.opacity,
        translate: rise.translate,
      }}
    >
      {label ? (
        <span
          style={{
            fontSize: 9.5,
            fontWeight: 600,
            textTransform: "uppercase",
            letterSpacing: "0.04em",
            color: t.ink3,
            padding: "0 4px",
          }}
        >
          {label}
        </span>
      ) : null}
      <div
        style={{
          maxWidth: "82%",
          padding: "10px 13px",
          borderRadius: 14,
          borderBottomRightRadius: user ? 5 : 14,
          borderBottomLeftRadius: user ? 14 : 5,
          fontSize: 12.5,
          lineHeight: 1.55,
          background: user ? t.active : t.paper1,
          color: user ? t.ink0 : t.ink1,
          border: `1px solid ${t.line1}`,
        }}
      >
        {children}
      </div>
    </div>
  );
};

/* Tool / diff / perm 區塊（r12 + 左 3px accent bar） */
const Block: React.FC<{
  from: number;
  bg: string;
  bd: string;
  bar: string;
  children: React.ReactNode;
}> = ({ from, bg, bd, bar, children }) => {
  const rise = useRise(from);
  return (
    <div
      style={{
        position: "relative",
        borderRadius: 12,
        background: bg,
        border: `1px solid ${bd}`,
        overflow: "hidden",
        opacity: rise.opacity,
        translate: rise.translate,
      }}
    >
      <div
        style={{
          position: "absolute",
          left: 0,
          top: 0,
          bottom: 0,
          width: 3,
          background: bar,
        }}
      />
      {children}
    </div>
  );
};

const ToolBlock: React.FC<{
  from: number;
  title: string;
  status: string;
  statusColor?: string;
}> = ({ from, title, status, statusColor = t.ink3 }) => (
  <Block from={from} bg={t.paper1} bd={t.line1} bar={t.accentInk}>
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 10,
        padding: "10px 12px",
        paddingLeft: 15,
      }}
    >
      <span
        style={{
          fontFamily: fonts.mono,
          fontSize: 11.5,
          fontWeight: 500,
          color: t.ink2,
          flex: 1,
        }}
      >
        {title}
      </span>
      <span style={{ fontFamily: fonts.mono, fontSize: 10.5, color: statusColor }}>
        {status}
      </span>
    </div>
  </Block>
);

const DiffBlock: React.FC<{ from: number }> = ({ from }) => {
  const frame = useCurrentFrame();
  const lines = [
    { s: "  let conn = pool.get().await?;", c: t.ink2 },
    { s: `- conn.query("SELECT ...")`, c: "#c2293f" },
    { s: `+ conn.query("SELECT ...")`, c: t.diffFg },
    { s: "+     .map_err(AppError::from)?;", c: t.diffFg },
  ];
  return (
    <Block from={from} bg={t.mintSoft} bd={t.diffBd} bar={t.diffFg}>
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 10,
          padding: "10px 12px 6px",
          paddingLeft: 15,
        }}
      >
        <span
          style={{
            fontFamily: fonts.mono,
            fontSize: 11.5,
            fontWeight: 600,
            color: t.diffFg,
            flex: 1,
          }}
        >
          src/db.rs
        </span>
        <span style={{ fontFamily: fonts.mono, fontSize: 10.5, color: t.diffFg }}>
          +12 −4
        </span>
      </div>
      <div
        style={{
          padding: "0 12px 10px 15px",
          fontFamily: fonts.mono,
          fontSize: 11,
          lineHeight: 1.6,
          whiteSpace: "pre",
        }}
      >
        {lines.map((l, i) => (
          <div
            key={i}
            style={{
              color: l.c,
              opacity: interpolate(frame, [from + 8 + i * 4, from + 16 + i * 4], [0, 1], {
                extrapolateLeft: "clamp",
                extrapolateRight: "clamp",
                easing: Easing.bezier(...EASE),
              }),
            }}
          >
            {l.s}
          </div>
        ))}
      </div>
    </Block>
  );
};

const PermBlock: React.FC<{
  from: number;
  pressAt: number;
  lang: Lang;
}> = ({ from, pressAt, lang }) => {
  const frame = useCurrentFrame();
  const c = COPY[lang];
  const pressed = frame >= pressAt;
  const dip = interpolate(frame, [pressAt, pressAt + 3, pressAt + 8], [1, 0.94, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  return (
    <Block from={from} bg={t.amberSoft} bd={t.permBd} bar={t.permFg}>
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 10,
          padding: "10px 12px",
          paddingLeft: 15,
        }}
      >
        <span
          style={{
            fontSize: 12,
            fontWeight: 600,
            color: t.permFg,
            flex: 1,
          }}
        >
          {c.perm}
        </span>
        <span
          style={{
            height: 26,
            padding: "0 12px",
            borderRadius: 8,
            background: pressed ? t.accentInk : t.accent,
            color: "#fff",
            fontSize: 11,
            fontWeight: 600,
            display: "inline-flex",
            alignItems: "center",
            boxShadow: t.shadowXs,
            scale: String(dip),
          }}
        >
          {c.allow}
          {pressed ? " ✓" : ""}
        </span>
        <span
          style={{
            height: 26,
            padding: "0 12px",
            borderRadius: 8,
            border: `1px solid ${t.line1}`,
            background: t.solid,
            color: t.ink1,
            fontSize: 11,
            fontWeight: 600,
            display: "inline-flex",
            alignItems: "center",
            opacity: pressed ? 0.45 : 1,
          }}
        >
          {c.deny}
        </span>
      </div>
    </Block>
  );
};

export const AgentZone: React.FC<{ lang: Lang }> = ({ lang }) => {
  const frame = useCurrentFrame();
  const { durationInFrames } = useVideoConfig();
  const c = COPY[lang];

  const T_USER = 16;
  const T_TOOL = 42;
  const T_TOOL_DONE = T_TOOL + 28;
  const T_REPLY = 84;
  const T_DIFF = 138;
  const T_PERM = 176;
  const T_PRESS = 208;
  const T_CHECK = 222;
  const T_CHECK_DONE = T_CHECK + 26;

  // 內容在片尾淡出 → 迴圈接回空 transcript
  const contentFade = interpolate(
    frame,
    [durationInFrames - 16, durationInFrames - 4],
    [1, 0],
    { extrapolateLeft: "clamp", extrapolateRight: "clamp" },
  );

  const sidebar = (
    <div style={{ display: "flex", flexDirection: "column", gap: 2 }}>
      <SideRow
        icon={
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round">
            <rect x="5" y="8" width="14" height="11" rx="3" />
            <path d="M12 8V4M8.5 13h.01M15.5 13h.01" />
          </svg>
        }
        label={c.session}
        active
      />
    </div>
  );

  const sidebarFooter = (
    <div style={{ display: "flex", gap: 8, alignItems: "center", marginTop: 10 }}>
      <div
        style={{
          height: 36,
          minWidth: 84,
          borderRadius: 11,
          border: `1px solid ${t.line1}`,
          background: t.solid,
          boxShadow: t.shadowXs,
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          padding: "0 10px",
          fontSize: 12.5,
          color: t.ink1,
        }}
      >
        claude <span style={{ color: t.ink3, fontSize: 9 }}>▾</span>
      </div>
      <div style={{ flex: 1 }}>
        <div
          style={{
            height: 36,
            borderRadius: 11,
            border: `1.5px dashed ${t.line2}`,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            gap: 6,
            color: t.ink3,
            fontSize: 12.5,
          }}
        >
          + {c.newSession}
        </div>
      </div>
    </div>
  );

  const main = (
    <>
      {/* session header */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 11,
          padding: "13px 16px",
          borderBottom: `1px solid ${t.line1}`,
          background: `linear-gradient(90deg, rgba(192,86,47,0.13), transparent 70%)`,
        }}
      >
        <div
          style={{
            width: 30,
            height: 30,
            borderRadius: 9,
            background: CLAUDE.color,
            color: "#fff",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            fontSize: 13,
            fontWeight: 700,
          }}
        >
          C
        </div>
        <span
          style={{
            fontFamily: fonts.serif,
            fontSize: 16,
            fontWeight: 600,
            letterSpacing: "-0.01em",
            color: t.ink0,
          }}
        >
          {c.session}
        </span>
        <span
          style={{
            height: 20,
            padding: "0 8px",
            borderRadius: 999,
            background: CLAUDE.bg,
            color: CLAUDE.color,
            fontSize: 10.5,
            fontWeight: 600,
            display: "inline-flex",
            alignItems: "center",
          }}
        >
          Claude
        </span>
        <span
          style={{
            fontFamily: fonts.mono,
            fontSize: 9.5,
            fontWeight: 700,
            padding: "2px 6px",
            borderRadius: 999,
            background: "rgba(91,63,209,0.13)",
            color: t.acpPurple,
          }}
        >
          ACP
        </span>
        <div style={{ flex: 1 }} />
        <span
          style={{
            height: 22,
            padding: "0 9px",
            borderRadius: 999,
            background: RUN.bg,
            color: RUN.fg,
            fontSize: 11,
            fontWeight: 600,
            display: "inline-flex",
            alignItems: "center",
            gap: 6,
          }}
        >
          <PulseDot color={RUN.dot} size={8} />
          {c.tone}
        </span>
      </div>

      {/* transcript */}
      <div
        style={{
          flex: 1,
          minHeight: 0,
          background: t.sunk,
          padding: "16px 18px",
          display: "flex",
          flexDirection: "column",
          gap: 12,
          opacity: contentFade,
          overflow: "hidden",
        }}
      >
        {frame >= T_USER ? (
          <Bubble from={T_USER} user label={c.you}>
            {c.user}
          </Bubble>
        ) : null}
        {frame >= T_TOOL ? (
          <ToolBlock
            from={T_TOOL}
            title={c.toolRead}
            status={frame >= T_TOOL_DONE ? c.done : c.running}
            statusColor={frame >= T_TOOL_DONE ? t.termOk : t.ink3}
          />
        ) : null}
        {frame >= T_REPLY ? (
          <Bubble from={T_REPLY} label="Claude">
            {typed(frame, c.reply, T_REPLY + 4)}
            {typedDone(frame, c.reply, T_REPLY + 4) ? null : (
              <Cursor color={t.acpPurple} />
            )}
          </Bubble>
        ) : null}
        {frame >= T_DIFF ? <DiffBlock from={T_DIFF} /> : null}
        {frame >= T_PERM ? (
          <PermBlock from={T_PERM} pressAt={T_PRESS} lang={lang} />
        ) : null}
        {frame >= T_CHECK ? (
          <ToolBlock
            from={T_CHECK}
            title={c.check}
            status={frame >= T_CHECK_DONE ? c.passed : c.running}
            statusColor={frame >= T_CHECK_DONE ? t.termOk : t.ink3}
          />
        ) : null}
      </div>

      {/* composer */}
      <div
        style={{
          display: "flex",
          gap: 9,
          alignItems: "center",
          padding: "11px 14px",
          borderTop: `1px solid ${t.line1}`,
          background: t.paper0,
        }}
      >
        <div
          style={{
            width: 38,
            height: 38,
            borderRadius: 11,
            background: t.field,
            border: `1px solid ${t.line2}`,
            boxShadow: t.shadowXs,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            color: t.ink3,
            fontFamily: fonts.mono,
            fontSize: 13,
          }}
        >
          /
        </div>
        <div
          style={{
            flex: 1,
            height: 38,
            borderRadius: 12,
            background: t.field,
            border: `1px solid ${t.line2}`,
            boxShadow: t.shadowXs,
            display: "flex",
            alignItems: "center",
            padding: "0 13px",
            color: t.ink3,
            fontSize: 13,
          }}
        >
          <span style={{ flex: 1 }}>{c.composer}</span>
          <span style={{ fontFamily: fonts.mono, fontSize: 10, color: t.ink4 }}>⌘↵</span>
        </div>
        <div
          style={{
            width: 38,
            height: 38,
            borderRadius: 11,
            background: t.ink1,
            color: t.paper0,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
          }}
        >
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <path d="M5 12h14M13 5l7 7-7 7" />
          </svg>
        </div>
      </div>
    </>
  );

  return (
    <Workbench
      mode={4}
      sectionLabel="AGENTZONE"
      sidebar={sidebar}
      sidebarFooter={sidebarFooter}
      main={main}
      lang={lang}
    />
  );
};
