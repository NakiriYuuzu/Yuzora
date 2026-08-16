import React from "react";
import { interpolate, useCurrentFrame, useVideoConfig } from "remotion";
import { fonts, t } from "./theme";
import { Cursor, typed, typedDone, useRise, useRowIn } from "./ui";
import { Workbench } from "./Workbench";

type Lang = "zh" | "en";

const COPY: Record<Lang, Record<string, string>> = {
  zh: {
    tabLog: "紀錄",
    tabLocal: "本地變更",
    tabConsole: "主控台",
    filter: "依訊息、作者或雜湊搜尋…",
    userAll: "使用者：全部",
    dateAll: "日期：全部",
    colCommit: "提交",
    colAuthor: "作者",
    colDate: "日期",
    changedFiles: "已變更的檔案",
    committed: "提交於",
    checkout: "簽出",
    compare: "比較",
    cherryPick: "揀選提交",
    clean: "工作目錄乾淨",
    commitPh: "提交訊息…",
    commit: "提交",
    terminal: "終端機",
  },
  en: {
    tabLog: "Log",
    tabLocal: "Local changes",
    tabConsole: "Console",
    filter: "Filter by message, author or hash…",
    userAll: "User: All",
    dateAll: "Date: All",
    colCommit: "COMMIT",
    colAuthor: "AUTHOR",
    colDate: "DATE",
    changedFiles: "Changed files",
    committed: "committed",
    checkout: "Checkout",
    compare: "Compare",
    cherryPick: "Cherry-pick",
    clean: "Working tree clean",
    commitPh: "Commit message…",
    commit: "Commit",
    terminal: "Terminal",
  },
};

/* ---- graph 幾何與色彩：對齊 app LogGraph.tsx / logColors.ts ---- */
const ROW_H = 32;
const GRAPH_W = 72;
const LANE_X = [18, 34];
const LANE_C = ["#3b6fe0", "#2bbf8a"];
const NODE_RING = "#fffdf8";
const SEL_BAR = "#3b6fe0";

/* ---- 提交資料（初始 6 列，兩條 lane） ---- */
const AUTHOR_C: Record<string, string> = {
  yuuzu: "#e08a3b",
  sora: "#2bbf8a",
  kenji: "#3b6fe0",
};

const CMD_PICK = "git cherry-pick f236785";
const PICKED_SUBJECT = "feat(ui): polish command palette";
const PICKED_OUT = "[main 9d4f2c1] feat(ui): polish command palette";
const PICKED_STAT = " 1 file changed, 24 insertions(+), 6 deletions(-)";
const CMD_PUSH = "git push";
const PUSH_OUT1 = "To github.com:NakiriYuuzu/Yuzora.git";
const PUSH_OUT2 = "   b7c6690..9d4f2c1  main -> main";

/* ---- timeline（30fps / 300f） ---- */
const ROWS_FROM = 10;
const DETAILS_IN = 26;
const T_PICK = 58;
const OUT_PICK = T_PICK + Math.ceil(CMD_PICK.length / 0.75) + 8;
const SYNC = OUT_PICK + 22; // graph 長出新 commit
const DETAIL_SWAP = SYNC + 8;
const AHEAD2 = SYNC + 12; // sidebar ↑1 → ↑2
const T_PUSH = 178;
const OUT_PUSH = T_PUSH + Math.ceil(CMD_PUSH.length / 0.75) + 8;
const SYNC2 = OUT_PUSH + 24; // origin/main chip 跟上、ahead 歸零

/* ---- RefChip：對齊 app RefChip（h17 r5 mono 9.5 semibold） ---- */
const CHIP_STYLE: Record<string, { bg: string; fg: string }> = {
  main: { bg: "#e7eeff", fg: "#2456cc" },
  feat: { bg: t.mintSoft, fg: "#0f7a55" },
  remote: { bg: "rgba(59, 111, 224, 0.10)", fg: "#5a7fd0" },
  tag: { bg: t.amberSoft, fg: "#9a6512" },
};

const Chip: React.FC<{
  label: string;
  kind: keyof typeof CHIP_STYLE;
  maxW: number;
  opacity?: number;
}> = ({ label, kind, maxW, opacity = 1 }) => (
  <span style={{ display: "inline-flex", maxWidth: maxW, overflow: "hidden", opacity }}>
    <span
      style={{
        height: 17,
        borderRadius: 5,
        background: CHIP_STYLE[kind].bg,
        color: CHIP_STYLE[kind].fg,
        fontFamily: fonts.mono,
        fontSize: 9.5,
        fontWeight: 600,
        padding: "0 6px",
        display: "inline-flex",
        alignItems: "center",
        whiteSpace: "nowrap",
      }}
    >
      {label}
    </span>
  </span>
);

/* ---- commit 列（graph 欄留白 72px，線與節點由外層 SVG 畫） ---- */
const Row: React.FC<{
  from: number;
  chips?: React.ReactNode;
  subject: string;
  author: string;
  time: string;
  selected?: boolean;
  glow?: number;
}> = ({ from, chips, subject, author, time, selected = false, glow = 0 }) => {
  const rowIn = useRowIn(from);
  return (
    <div
      style={{
        height: ROW_H,
        display: "flex",
        alignItems: "center",
        background: glow > 0
          ? `rgba(${t.accentRgb}, ${glow})`
          : selected
            ? t.active
            : "transparent",
        boxShadow: selected ? `inset 2px 0 0 ${SEL_BAR}` : "none",
      }}
    >
      <span style={{ width: GRAPH_W, flex: "none" }} />
      <span
        style={{
          flex: 1,
          minWidth: 0,
          display: "flex",
          alignItems: "center",
          gap: 6,
          opacity: rowIn.opacity,
          translate: rowIn.translate,
        }}
      >
        {chips}
        <span
          style={{
            fontSize: 12.5,
            color: t.ink1,
            whiteSpace: "nowrap",
            overflow: "hidden",
            textOverflow: "ellipsis",
          }}
        >
          {subject}
        </span>
      </span>
      <span
        style={{
          width: 76,
          flex: "none",
          display: "flex",
          alignItems: "center",
          gap: 5,
          opacity: rowIn.opacity,
        }}
      >
        <span
          style={{
            width: 7,
            height: 7,
            borderRadius: "50%",
            background: AUTHOR_C[author],
            flex: "none",
          }}
        />
        <span style={{ fontSize: 11, color: t.ink2, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
          {author}
        </span>
      </span>
      <span
        style={{
          width: 44,
          flex: "none",
          fontFamily: fonts.mono,
          fontSize: 10.5,
          color: t.ink3,
          textAlign: "right",
          paddingRight: 12,
          opacity: rowIn.opacity,
        }}
      >
        {time}
      </span>
    </div>
  );
};

/* ---- 節點：對齊 app（r 4.5 / 有 refs 5.2、ring 2px） ---- */
const Node: React.FC<{ x: number; y: number; c: string; r: number; popAt: number }> = ({
  x,
  y,
  c,
  r,
  popAt,
}) => {
  const frame = useCurrentFrame();
  const s = interpolate(frame, [popAt, popAt + 7], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  return <circle cx={x} cy={y} r={r * s} fill={c} stroke={NODE_RING} strokeWidth={2 * s} />;
};

/* ---- CommitDetails 內容（右欄 240px） ---- */
const Details: React.FC<{
  lang: Lang;
  hash: string;
  subject: string;
  author: string;
  when: string;
  files: [string, string][];
}> = ({ lang, hash, subject, author, when, files }) => {
  const c = COPY[lang];
  return (
    <div style={{ padding: "14px 14px 12px", display: "flex", flexDirection: "column", gap: 10 }}>
      <div style={{ display: "flex", alignItems: "center", gap: 7 }}>
        <span
          style={{
            height: 20,
            borderRadius: 6,
            background: "#e7eeff",
            color: "#2456cc",
            fontFamily: fonts.mono,
            fontSize: 11,
            fontWeight: 600,
            padding: "0 8px",
            display: "inline-flex",
            alignItems: "center",
          }}
        >
          {hash}
        </span>
        <span style={{ color: t.ink4, fontSize: 11 }}>⧉</span>
      </div>
      <div
        style={{
          fontFamily: fonts.serif,
          fontSize: 14.5,
          fontWeight: 600,
          color: t.ink0,
          lineHeight: 1.35,
        }}
      >
        {subject}
      </div>
      <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
        <span
          style={{
            width: 26,
            height: 26,
            borderRadius: "50%",
            background: AUTHOR_C[author],
            color: "#fff",
            fontSize: 10.5,
            fontWeight: 700,
            display: "inline-flex",
            alignItems: "center",
            justifyContent: "center",
          }}
        >
          {author.slice(0, 1).toUpperCase()}
        </span>
        <span style={{ fontSize: 12, color: t.ink1, fontWeight: 600 }}>{author}</span>
      </div>
      <div style={{ fontFamily: fonts.mono, fontSize: 10.5, color: t.ink3 }}>
        {c.committed} {when}
      </div>
      <div style={{ height: 1, background: t.line1 }} />
      <div
        style={{
          fontSize: 9.5,
          fontWeight: 700,
          letterSpacing: "0.07em",
          textTransform: "uppercase",
          color: t.ink3,
        }}
      >
        {c.changedFiles}
      </div>
      <div style={{ display: "flex", flexDirection: "column", gap: 5 }}>
        {files.map(([f, stat]) => (
          <div key={f} style={{ display: "flex", alignItems: "center", gap: 7 }}>
            <span
              style={{
                width: 15,
                height: 15,
                borderRadius: 4,
                background: "#e7eeff",
                color: "#2456cc",
                fontFamily: fonts.mono,
                fontSize: 9,
                fontWeight: 700,
                display: "inline-flex",
                alignItems: "center",
                justifyContent: "center",
                flex: "none",
              }}
            >
              M
            </span>
            <span
              style={{
                flex: 1,
                minWidth: 0,
                fontFamily: fonts.mono,
                fontSize: 10.5,
                color: t.ink1,
                overflow: "hidden",
                textOverflow: "ellipsis",
                whiteSpace: "nowrap",
              }}
            >
              {f}
            </span>
            <span style={{ fontFamily: fonts.mono, fontSize: 10, color: t.ink3, flex: "none" }}>
              {stat}
            </span>
          </div>
        ))}
      </div>
      <div style={{ display: "flex", gap: 6, flexWrap: "wrap", marginTop: 2 }}>
        {[c.checkout, c.compare, c.cherryPick].map((b) => (
          <span
            key={b}
            style={{
              height: 24,
              borderRadius: 8,
              border: `1px solid ${t.line1}`,
              background: t.solid,
              boxShadow: t.shadowXs,
              padding: "0 9px",
              fontSize: 11,
              fontWeight: 500,
              color: t.ink1,
              display: "inline-flex",
              alignItems: "center",
            }}
          >
            {b}
          </span>
        ))}
      </div>
    </div>
  );
};

export const TerminalGit: React.FC<{ lang: Lang }> = ({ lang }) => {
  const frame = useCurrentFrame();
  const { durationInFrames } = useVideoConfig();
  const c = COPY[lang];

  const pickDone = typedDone(frame, CMD_PICK, T_PICK, 0.75);
  const pushDone = typedDone(frame, CMD_PUSH, T_PUSH, 0.75);

  /* 鏡頭緩推：整段 1 → 1.03（render 需配 --crf=28） */
  const cam = interpolate(frame, [0, durationInFrames], [1, 1.03]);
  const contentFade = interpolate(
    frame,
    [durationInFrames - 16, durationInFrames - 4],
    [1, 0],
    { extrapolateLeft: "clamp", extrapolateRight: "clamp" },
  );

  /* ---- 新 commit 插入：列高 0→32、綠光衰減 ---- */
  const insertH = interpolate(frame, [SYNC, SYNC + 10], [0, ROW_H], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  const newGlow = interpolate(frame, [SYNC + 8, SYNC + 48], [0.16, 0], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  /* lane0 主線向上長到新節點 */
  const laneTop = interpolate(frame, [SYNC, SYNC + 10], [16, 0], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  /* 初始 graph 線條 draw-in */
  const lane0Draw = interpolate(frame, [12, 38], [160, 0], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  const lane1Draw = interpolate(frame, [20, 48], [112, 0], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

  /* chips 搬家 */
  const mainChipOld = interpolate(frame, [SYNC + 2, SYNC + 10], [44, 0], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  const mainChipNew = interpolate(frame, [SYNC + 6, SYNC + 13], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  const remoteChipOld = interpolate(frame, [SYNC2, SYNC2 + 8], [86, 0], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  const remoteChipNew = interpolate(frame, [SYNC2 + 6, SYNC2 + 14], [0, 86], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

  const selNew = frame >= DETAIL_SWAP;
  const detailOut = interpolate(frame, [DETAIL_SWAP, DETAIL_SWAP + 5], [1, 0], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  const detailIn = interpolate(frame, [DETAIL_SWAP + 5, DETAIL_SWAP + 13], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  const detailsIn = useRise(DETAILS_IN, 10);

  /* sidebar ahead：↑1 → ↑2 → 歸零 */
  const aheadRise = useRise(AHEAD2, 7);
  const syncedRise = useRise(SYNC2 + 10, 8);
  const aheadGone = frame >= SYNC2 + 10;

  /* ---- 主區：GitPanel（紀錄 tab + graph + details） ---- */
  const main = (
    <>
      {/* toolbar h43：line-style tabs + branch pill + fetch/pull/push */}
      <div
        style={{
          height: 43,
          display: "flex",
          alignItems: "center",
          gap: 2,
          padding: "0 12px",
          borderBottom: `1px solid ${t.line1}`,
          flex: "none",
        }}
      >
        {[
          { label: c.tabLog, active: true },
          { label: c.tabLocal, active: false },
          { label: c.tabConsole, active: false },
        ].map((tab) => (
          <span
            key={tab.label}
            style={{
              position: "relative",
              height: 43,
              display: "inline-flex",
              alignItems: "center",
              padding: "0 10px",
              fontSize: 12.5,
              fontWeight: tab.active ? 600 : 500,
              color: tab.active ? t.ink0 : t.ink3,
            }}
          >
            {tab.label}
            {tab.active ? (
              <span
                style={{
                  position: "absolute",
                  left: 10,
                  right: 10,
                  bottom: 0,
                  height: 2,
                  borderRadius: 2,
                  background: t.accent,
                }}
              />
            ) : null}
          </span>
        ))}
        <div style={{ flex: 1 }} />
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
            padding: "0 10px",
            fontFamily: fonts.mono,
            fontSize: 11.5,
            fontWeight: 500,
            color: t.ink1,
          }}
        >
          <span style={{ width: 8, height: 8, borderRadius: "50%", background: t.accent }} />
          main
          <span style={{ color: t.ink3, fontSize: 9 }}>▾</span>
        </span>
        {["↻", "↓", "↑"].map((g, i) => {
          const pushHot = i === 2 && frame >= T_PUSH && frame < SYNC2;
          return (
            <span
              key={g}
              style={{
                width: 26,
                height: 26,
                borderRadius: 8,
                marginLeft: 4,
                display: "inline-flex",
                alignItems: "center",
                justifyContent: "center",
                fontSize: 12.5,
                background: pushHot ? `rgba(${t.accentRgb}, 0.18)` : "transparent",
                color: pushHot ? t.accentInk : t.ink3,
              }}
            >
              {g}
            </span>
          );
        })}
      </div>
      {/* filter row h40 */}
      <div
        style={{
          height: 40,
          display: "flex",
          alignItems: "center",
          gap: 8,
          padding: "0 12px",
          borderBottom: `1px solid ${t.line1}`,
          flex: "none",
        }}
      >
        <div
          style={{
            flex: 1,
            height: 27,
            borderRadius: 8,
            border: `1px solid ${t.line1}`,
            background: t.field,
            display: "flex",
            alignItems: "center",
            gap: 7,
            padding: "0 9px",
            color: t.ink3,
            fontSize: 11.5,
          }}
        >
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke={t.ink3} strokeWidth="1.8" strokeLinecap="round">
            <circle cx="11" cy="11" r="6.5" />
            <path d="M20 20l-3.5-3.5" />
          </svg>
          {c.filter}
        </div>
        {[c.userAll, c.dateAll].map((d) => (
          <span
            key={d}
            style={{
              height: 26,
              borderRadius: 8,
              border: `1px solid ${t.line1}`,
              background: t.solid,
              padding: "0 9px",
              fontSize: 11,
              color: t.ink2,
              display: "inline-flex",
              alignItems: "center",
              gap: 5,
              flex: "none",
            }}
          >
            {d}
            <span style={{ color: t.ink4, fontSize: 8.5 }}>▾</span>
          </span>
        ))}
      </div>
      {/* body：graph 列表 + 240px details */}
      <div style={{ flex: 1, minHeight: 0, display: "flex" }}>
        <div style={{ flex: 1, minWidth: 0, display: "flex", flexDirection: "column" }}>
          {/* 欄位表頭 h28 */}
          <div
            style={{
              height: 28,
              display: "flex",
              alignItems: "center",
              borderBottom: `1px solid ${t.line1}`,
              fontSize: 9.5,
              fontWeight: 700,
              letterSpacing: "0.07em",
              textTransform: "uppercase",
              color: t.ink3,
              flex: "none",
            }}
          >
            <span style={{ width: GRAPH_W, flex: "none" }} />
            <span style={{ flex: 1 }}>{c.colCommit}</span>
            <span style={{ width: 76, flex: "none" }}>{c.colAuthor}</span>
            <span style={{ width: 44, flex: "none", textAlign: "right", paddingRight: 12 }}>
              {c.colDate}
            </span>
          </div>
          {/* commit rows */}
          <div style={{ flex: 1, minHeight: 0, overflow: "hidden", position: "relative" }}>
            {/* 新 commit：cherry-pick 後長出來 */}
            <div style={{ height: insertH, overflow: "hidden" }}>
              <div style={{ position: "relative" }}>
                <svg
                  width={GRAPH_W}
                  height={ROW_H}
                  style={{ position: "absolute", left: 0, top: 0, pointerEvents: "none" }}
                >
                  <path d={`M${LANE_X[0]} 16 V${ROW_H}`} stroke={LANE_C[0]} strokeWidth={2} strokeLinecap="round" fill="none" />
                  <Node x={LANE_X[0]} y={16} c={LANE_C[0]} r={5.2} popAt={SYNC + 6} />
                </svg>
                <Row
                  from={SYNC + 4}
                  chips={
                    <>
                      <Chip label="main" kind="main" maxW={44} opacity={mainChipNew} />
                      <Chip label="origin/main" kind="remote" maxW={remoteChipNew} opacity={remoteChipNew > 2 ? 1 : 0} />
                    </>
                  }
                  subject={PICKED_SUBJECT}
                  author="sora"
                  time="now"
                  selected={selNew}
                  glow={newGlow}
                />
              </div>
            </div>
            {/* 初始 6 列 + 整片 graph SVG */}
            <div style={{ position: "relative" }}>
              <svg
                width={GRAPH_W}
                height={ROW_H * 6}
                style={{ position: "absolute", left: 0, top: 0, pointerEvents: "none" }}
              >
                {/* lane0 主線（cherry-pick 後向上接到新節點） */}
                <path
                  d={`M${LANE_X[0]} ${laneTop} V176`}
                  stroke={LANE_C[0]}
                  strokeWidth={2}
                  strokeLinecap="round"
                  fill="none"
                  strokeDasharray={176}
                  strokeDashoffset={lane0Draw}
                />
                {/* lane1 feat 分支 + cubic 併回 lane0（app 同款彎線） */}
                <path
                  d={`M${LANE_X[1]} 48 V112 C${LANE_X[1]} 128 ${LANE_X[0]} 128 ${LANE_X[0]} 144`}
                  stroke={LANE_C[1]}
                  strokeWidth={2}
                  strokeLinecap="round"
                  fill="none"
                  strokeDasharray={130}
                  strokeDashoffset={lane1Draw}
                />
                <Node x={LANE_X[0]} y={16} c={LANE_C[0]} r={5.2} popAt={ROWS_FROM + 2} />
                <Node x={LANE_X[1]} y={48} c={LANE_C[1]} r={5.2} popAt={ROWS_FROM + 7} />
                <Node x={LANE_X[0]} y={80} c={LANE_C[0]} r={5.2} popAt={ROWS_FROM + 12} />
                <Node x={LANE_X[1]} y={112} c={LANE_C[1]} r={4.5} popAt={ROWS_FROM + 17} />
                <Node x={LANE_X[0]} y={144} c={LANE_C[0]} r={5.2} popAt={ROWS_FROM + 22} />
                <Node x={LANE_X[0]} y={176} c={LANE_C[0]} r={4.5} popAt={ROWS_FROM + 27} />
              </svg>
              <Row
                from={ROWS_FROM}
                chips={<Chip label="main" kind="main" maxW={mainChipOld} />}
                subject="fix(windows): extract managed Herdr with PowerShell"
                author="yuuzu"
                time="2h"
                selected={!selNew}
              />
              <Row
                from={ROWS_FROM + 5}
                chips={<Chip label="feat/palette" kind="feat" maxW={84} />}
                subject={PICKED_SUBJECT}
                author="sora"
                time="1d"
              />
              <Row
                from={ROWS_FROM + 10}
                chips={<Chip label="origin/main" kind="remote" maxW={remoteChipOld} />}
                subject="fix(build): make Herdr resource fetch portable"
                author="yuuzu"
                time="3d"
              />
              <Row
                from={ROWS_FROM + 15}
                subject="feat(ui): scaffold palette list"
                author="sora"
                time="4d"
              />
              <Row
                from={ROWS_FROM + 20}
                chips={<Chip label="v0.0.8" kind="tag" maxW={52} />}
                subject="chore(release): v0.0.8 candidates"
                author="yuuzu"
                time="1w"
              />
              <Row
                from={ROWS_FROM + 25}
                subject="chore(repo): quality toolchain + refactor"
                author="kenji"
                time="2w"
              />
            </div>
          </div>
        </div>
        {/* CommitDetails 240px */}
        <div
          style={{
            width: 240,
            flex: "none",
            borderLeft: `1px solid ${t.line1}`,
            position: "relative",
            opacity: detailsIn.opacity,
            translate: detailsIn.translate,
          }}
        >
          <div style={{ position: "absolute", inset: 0, opacity: detailOut }}>
            <Details
              lang={lang}
              hash="b7c6690"
              subject="fix(windows): extract managed Herdr with PowerShell"
              author="yuuzu"
              when="2026-08-16 09:32"
              files={[
                ["herdr_service.rs", "+18 −4"],
                ["setup.ps1", "+6 −1"],
              ]}
            />
          </div>
          <div style={{ position: "absolute", inset: 0, opacity: detailIn }}>
            <Details
              lang={lang}
              hash="9d4f2c1"
              subject={PICKED_SUBJECT}
              author="sora"
              when="2026-08-16 11:41"
              files={[["CommandPalette.tsx", "+24 −6"]]}
            />
          </div>
        </div>
      </div>
    </>
  );

  /* ---- sidebar：GitNavContent（branch 狀態卡 + 乾淨工作目錄 + composer） ---- */
  const sidebar = (
    <div style={{ display: "flex", flexDirection: "column", gap: 12, height: "100%" }}>
      <div
        style={{
          borderRadius: 12,
          border: `1px solid rgba(${t.accentRgb}, 0.36)`,
          background: t.panel,
          padding: "10px 11px",
          display: "flex",
          flexDirection: "column",
          gap: 6,
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 7 }}>
          <span style={{ width: 8, height: 8, borderRadius: "50%", background: t.accent }} />
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke={t.ink2} strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
            <circle cx="6" cy="5" r="2.4" />
            <circle cx="6" cy="19" r="2.4" />
            <circle cx="18" cy="9" r="2.4" />
            <path d="M6 7.5v9M18 11.5a7 7 0 0 1-7 5.5H6" />
          </svg>
          <span style={{ fontFamily: fonts.mono, fontSize: 12, fontWeight: 600, color: t.ink0 }}>
            main
          </span>
          <div style={{ flex: 1 }} />
          <span style={{ color: t.ink3, fontSize: 9 }}>▾</span>
        </div>
        <div style={{ fontFamily: fonts.mono, fontSize: 10.5, color: t.ink3, paddingLeft: 15 }}>
          {aheadGone ? (
            <span style={{ opacity: syncedRise.opacity, translate: syncedRise.translate, display: "inline-flex", gap: 5 }}>
              <span style={{ color: t.termOk }}>✓</span> origin/main
            </span>
          ) : frame >= AHEAD2 ? (
            <span style={{ opacity: aheadRise.opacity, translate: aheadRise.translate, display: "inline-flex", gap: 5 }}>
              <span style={{ color: "#2456cc" }}>↑2</span> origin/main
            </span>
          ) : (
            <span style={{ display: "inline-flex", gap: 5 }}>
              <span style={{ color: "#2456cc" }}>↑1</span> origin/main
            </span>
          )}
        </div>
      </div>
      <div
        style={{
          flex: 1,
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          justifyContent: "center",
          gap: 8,
        }}
      >
        <span
          style={{
            width: 30,
            height: 30,
            borderRadius: "50%",
            background: t.mintSoft,
            color: t.termOk,
            fontSize: 13,
            display: "inline-flex",
            alignItems: "center",
            justifyContent: "center",
          }}
        >
          ✓
        </span>
        <span style={{ fontSize: 12, color: t.ink3 }}>{c.clean}</span>
      </div>
      <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
        <div
          style={{
            height: 52,
            borderRadius: 10,
            border: `1px solid ${t.line1}`,
            background: t.solid,
            padding: "8px 10px",
            fontSize: 12,
            color: t.ink3,
          }}
        >
          {c.commitPh}
        </div>
        <div
          style={{
            height: 30,
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
    </div>
  );

  /* ---- terminal drawer ---- */
  const drawer = (
    <div
      style={{
        height: 250,
        borderRadius: 20,
        border: `1px solid ${t.termLine}`,
        background: t.termBg,
        boxShadow: t.shadowLg,
        overflow: "hidden",
        display: "flex",
        flexDirection: "column",
        flex: "none",
      }}
    >
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
      <div
        style={{
          height: 36,
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
          workspace/yuzora
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
          {typed(frame, CMD_PICK, T_PICK, 0.75)}
          {pickDone ? null : <Cursor color={t.termFg} width={7} height={13} />}
        </div>
        {frame >= OUT_PICK ? <GlowLine from={OUT_PICK} text={PICKED_OUT} color={t.termOk} /> : null}
        {frame >= OUT_PICK + 6 ? (
          <FadeLine from={OUT_PICK + 6} text={PICKED_STAT} color={t.termFg2} />
        ) : null}
        {frame >= T_PUSH ? (
          <div>
            <span style={{ color: t.termFg2 }}>yuzora % </span>
            {typed(frame, CMD_PUSH, T_PUSH, 0.75)}
            {pushDone ? null : <Cursor color={t.termFg} width={7} height={13} />}
          </div>
        ) : null}
        {frame >= OUT_PUSH ? <FadeLine from={OUT_PUSH} text={PUSH_OUT1} color={t.termFg2} /> : null}
        {frame >= OUT_PUSH + 6 ? (
          <GlowLine from={OUT_PUSH + 6} text={PUSH_OUT2} color={t.termBlue} />
        ) : null}
        {frame >= OUT_PUSH + 22 ? (
          <div>
            <span style={{ color: t.termFg2 }}>yuzora % </span>
            <Cursor color={t.termFg} width={7} height={13} />
          </div>
        ) : null}
      </div>
    </div>
  );

  return (
    <div
      style={{
        width: "100%",
        height: "100%",
        transform: `scale(${cam})`,
        transformOrigin: "50% 42%",
      }}
    >
      <Workbench mode={1} sectionLabel="GIT" sidebar={sidebar} main={main} drawer={drawer} lang={lang} />
    </div>
  );
};

/* 輸出列：進場 + 短暫 highlight 衰減 */
const GlowLine: React.FC<{ from: number; text: string; color: string }> = ({
  from,
  text,
  color,
}) => {
  const frame = useCurrentFrame();
  const rise = useRise(from, 8);
  const glow = interpolate(frame, [from, from + 26], [0.16, 0], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  return (
    <div
      style={{
        opacity: rise.opacity,
        color,
        fontWeight: 600,
        background: `rgba(31, 138, 91, ${glow})`,
        borderRadius: 6,
        padding: "0 8px",
        margin: "0 -8px",
      }}
    >
      {text}
    </div>
  );
};

const FadeLine: React.FC<{ from: number; text: string; color: string }> = ({
  from,
  text,
  color,
}) => {
  const rise = useRise(from, 8);
  return <div style={{ opacity: rise.opacity, color }}>{text}</div>;
};
