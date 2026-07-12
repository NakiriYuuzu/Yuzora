import React from "react";
import { interpolate, useCurrentFrame, useVideoConfig } from "remotion";
import { fonts, t } from "./theme";
import { Cursor, typed, typedDone, useRise, useRowIn } from "./ui";
import { Workbench } from "./Workbench";

type Lang = "zh" | "en";

const COPY: Record<Lang, Record<string, string>> = {
  zh: {
    changed: "CHANGED",
    commitPlaceholder: "提交訊息…",
    commit: "提交",
    terminal: "終端機",
  },
  en: {
    changed: "CHANGED",
    commitPlaceholder: "Commit message…",
    commit: "Commit",
    terminal: "Terminal",
  },
};

const CMD1 = "git log --oneline -3";
const LOG = [
  ["0809c9b", "chore(docs): remove evaluation drafts"],
  ["f236785", "chore(repo): quality toolchain + refactor"],
  ["ddbb9fc", "chore(core): wire M4/M5 commands"],
];
const CMD2 = "git cherry-pick f236785";
const PICKED = "[main 4c81f2a] chore(repo): quality toolchain + refactor";

/* GitBadge：M 藍色 chip（--status-m / --blue-soft） */
const MBadge: React.FC = () => (
  <span
    style={{
      width: 16,
      height: 16,
      borderRadius: 5,
      background: "#e7eeff",
      color: "#2456cc",
      fontFamily: fonts.mono,
      fontSize: 9.5,
      fontWeight: 700,
      display: "inline-flex",
      alignItems: "center",
      justifyContent: "center",
    }}
  >
    M
  </span>
);

const CODE_LINES: { s: string; k?: boolean }[] = [
  { s: "pub async fn connect(&self) -> Result<Client> {" },
  { s: "    let tls = MakeTlsConnector::new(builder.build()?);", k: true },
  { s: "    let (client, conn) = config.connect(tls).await?;" },
  { s: "    tokio::spawn(conn);" },
  { s: "    Ok(client)" },
  { s: "}" },
];

export const TerminalGit: React.FC<{ lang: Lang }> = ({ lang }) => {
  const frame = useCurrentFrame();
  const { durationInFrames } = useVideoConfig();
  const c = COPY[lang];

  const T1 = 22;
  const cmd1Done = typedDone(frame, CMD1, T1, 0.75);
  const LOG_FROM = T1 + Math.ceil(CMD1.length / 0.75) + 8;
  const T2 = LOG_FROM + 40;
  const cmd2Done = typedDone(frame, CMD2, T2, 0.75);
  const PICKED_FROM = T2 + Math.ceil(CMD2.length / 0.75) + 8;

  const contentFade = interpolate(
    frame,
    [durationInFrames - 16, durationInFrames - 4],
    [1, 0],
    { extrapolateLeft: "clamp", extrapolateRight: "clamp" },
  );

  /* ---- sidebar：GIT mode（commit 卡 + changed 清單） ---- */
  const sidebar = (
    <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
      <div
        style={{
          borderRadius: 12,
          border: `1px solid ${t.line1}`,
          background: t.panel,
          padding: 11,
          display: "flex",
          flexDirection: "column",
          gap: 9,
        }}
      >
        <div style={{ display: "flex", gap: 7, alignItems: "center" }}>
          <span
            style={{
              height: 26,
              borderRadius: 999,
              border: `1px solid ${t.line1}`,
              background: t.solid,
              boxShadow: t.shadowXs,
              display: "inline-flex",
              alignItems: "center",
              gap: 6,
              padding: "0 11px 0 10px",
              fontFamily: fonts.mono,
              fontSize: 11.5,
              fontWeight: 500,
              color: t.ink1,
            }}
          >
            <span
              style={{
                width: 8,
                height: 8,
                borderRadius: "50%",
                background: t.accent,
              }}
            />
            main
          </span>
          <span
            style={{
              height: 22,
              borderRadius: 999,
              background: t.amberSoft,
              color: "#9a6512",
              padding: "0 9px",
              fontSize: 11.5,
              fontWeight: 600,
              display: "inline-flex",
              alignItems: "center",
              gap: 5,
            }}
          >
            <span style={{ width: 6, height: 6, background: "#d68a0c", borderRadius: 1.5 }} />
            2
          </span>
        </div>
        <div style={{ height: 1, background: t.line1 }} />
        <div
          style={{
            height: 32,
            borderRadius: 9,
            border: `1px solid ${t.line1}`,
            background: t.solid,
            display: "flex",
            alignItems: "center",
            padding: "0 11px",
            fontSize: 12,
            color: t.ink3,
          }}
        >
          {c.commitPlaceholder}
        </div>
        <div
          style={{
            height: 32,
            borderRadius: 9,
            background: t.paper3,
            color: t.ink4,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            fontSize: 11.5,
            fontWeight: 600,
          }}
        >
          {c.commit}
        </div>
      </div>
      <div
        style={{
          fontSize: 10.5,
          fontWeight: 600,
          letterSpacing: "0.07em",
          color: t.ink3,
          padding: "0 4px",
        }}
      >
        {c.changed}
      </div>
      <div style={{ display: "flex", flexDirection: "column", gap: 1 }}>
        {["db_service.rs", "lib.rs"].map((f) => (
          <div
            key={f}
            style={{
              height: 30,
              borderRadius: 8,
              display: "flex",
              alignItems: "center",
              gap: 9,
              padding: "0 8px",
              color: t.ink1,
            }}
          >
            <MBadge />
            <span style={{ fontSize: 12.5, fontWeight: 500 }}>
              {f}
              <span style={{ fontSize: 10.5, color: t.ink4, marginLeft: 6 }}>src-tauri/src</span>
            </span>
          </div>
        ))}
      </div>
    </div>
  );

  /* ---- main：editor（tab bar + code） ---- */
  const main = (
    <>
      <div
        style={{
          height: 44,
          display: "flex",
          alignItems: "center",
          gap: 3,
          padding: "0 8px",
          borderBottom: `1px solid ${t.line1}`,
        }}
      >
        {[
          { name: "db_service.rs", active: true },
          { name: "lib.rs", active: false },
        ].map((tab) => (
          <span
            key={tab.name}
            style={{
              height: 30,
              borderRadius: 9,
              padding: "0 8px 0 12px",
              display: "inline-flex",
              alignItems: "center",
              gap: 8,
              background: tab.active ? t.active : "transparent",
              boxShadow: tab.active ? t.shadowXs : "none",
              color: tab.active ? t.ink0 : t.ink3,
              fontSize: 12.5,
              fontWeight: tab.active ? 600 : 500,
            }}
          >
            <span style={{ width: 7, height: 7, borderRadius: "50%", background: "#d68a0c" }} />
            {tab.name}
            <span style={{ color: t.ink3, fontSize: 11 }}>×</span>
          </span>
        ))}
      </div>
      <div
        style={{
          flex: 1,
          padding: "14px 16px",
          fontFamily: fonts.mono,
          fontSize: 12.5,
          lineHeight: 1.85,
          color: t.ink2,
          whiteSpace: "pre",
          overflow: "hidden",
        }}
      >
        {CODE_LINES.map((l, i) => (
          <div key={i}>
            <span style={{ color: t.ink4, marginRight: 16 }}>{i + 1}</span>
            <span style={{ color: l.k ? t.ink0 : t.ink2 }}>{l.s}</span>
          </div>
        ))}
      </div>
    </>
  );

  /* ---- terminal drawer ---- */
  const drawer = (
    <div
      style={{
        height: 280,
        borderRadius: 20,
        border: `1px solid ${t.termLine}`,
        background: t.termBg,
        boxShadow: t.shadowLg,
        overflow: "hidden",
        display: "flex",
        flexDirection: "column",
      }}
    >
      {/* resize handle */}
      <div
        style={{
          height: 6,
          background: t.termBar,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
        }}
      >
        <span style={{ width: 34, height: 3, borderRadius: 999, background: t.termFg2, opacity: 0.5 }} />
      </div>
      {/* header */}
      <div
        style={{
          height: 38,
          background: t.termBar,
          display: "flex",
          alignItems: "center",
          gap: 8,
          padding: "0 11px",
        }}
      >
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke={t.termFg2} strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
          <path d="M4 17l4-5-4-5M11 17h9" />
        </svg>
        <span style={{ fontSize: 12, fontWeight: 600, color: t.termFg }}>{c.terminal}</span>
        <span style={{ fontFamily: fonts.mono, fontSize: 10, color: t.termFg2 }}>
          ~/App/Tauri/yuzora
        </span>
        <div style={{ flex: 1 }} />
        {["⊞", "⊟", "+", "×"].map((s, i) => (
          <span
            key={i}
            style={{
              width: 24,
              height: 24,
              borderRadius: 7,
              display: "inline-flex",
              alignItems: "center",
              justifyContent: "center",
              color: t.termFg2,
              fontSize: 12,
            }}
          >
            {s}
          </span>
        ))}
      </div>
      {/* body */}
      <div
        style={{
          flex: 1,
          padding: "10px 14px",
          fontFamily: fonts.mono,
          fontSize: 12,
          lineHeight: 1.9,
          color: t.termFg,
          whiteSpace: "pre-wrap",
          opacity: contentFade,
        }}
      >
        <div>
          <span style={{ color: t.termFg2 }}>yuzora % </span>
          {typed(frame, CMD1, T1, 0.75)}
          {cmd1Done ? null : <Cursor color={t.termFg} width={7} height={13} />}
        </div>
        {frame >= LOG_FROM
          ? LOG.map(([hash, msg], i) => (
              <LogLine key={hash} hash={hash} msg={msg} from={LOG_FROM + i * 6} />
            ))
          : null}
        {frame >= T2 ? (
          <div>
            <span style={{ color: t.termFg2 }}>yuzora % </span>
            {typed(frame, CMD2, T2, 0.75)}
            {cmd2Done ? null : <Cursor color={t.termFg} width={7} height={13} />}
          </div>
        ) : null}
        {frame >= PICKED_FROM ? <PickedLine from={PICKED_FROM} /> : null}
        {frame >= PICKED_FROM + 14 ? (
          <div>
            <span style={{ color: t.termFg2 }}>yuzora % </span>
            <Cursor color={t.termFg} width={7} height={13} />
          </div>
        ) : null}
      </div>
    </div>
  );

  return (
    <Workbench
      mode={1}
      sectionLabel="GIT"
      sidebar={sidebar}
      main={main}
      drawer={drawer}
      lang={lang}
    />
  );
};

const LogLine: React.FC<{ hash: string; msg: string; from: number }> = ({
  hash,
  msg,
  from,
}) => {
  const rowIn = useRowIn(from);
  return (
    <div style={{ opacity: rowIn.opacity, translate: rowIn.translate }}>
      <span style={{ color: t.termCoral, fontWeight: 600 }}>{hash}</span> {msg}
    </div>
  );
};

const PickedLine: React.FC<{ from: number }> = ({ from }) => {
  const rise = useRise(from, 8);
  return (
    <div style={{ opacity: rise.opacity, color: t.termOk }}>{PICKED}</div>
  );
};
