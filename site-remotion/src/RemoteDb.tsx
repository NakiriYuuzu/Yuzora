import React from "react";
import {
  Easing,
  interpolate,
  useCurrentFrame,
  useVideoConfig,
} from "remotion";
import { fonts, t, EASE } from "./theme";
import { Cursor, typed, typedDone, useRowIn } from "./ui";
import { DashedButton, SideRow, Workbench } from "./Workbench";

type Lang = "zh" | "en";

const QUERY = "SELECT * FROM orders LIMIT 3;";

const COLS = ["id", "status", "total", "created_at"];
const ROWS = [
  ["10241", "paid", "1,280", "2026-07-09 21:14"],
  ["10242", "pending", "640", "2026-07-09 21:20"],
  ["10243", "paid", "2,150", "2026-07-09 21:31"],
];
const COL_W = [90, 130, 110, 200];

const DbIcon = (
  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round">
    <ellipse cx="12" cy="5.5" rx="8" ry="3" />
    <path d="M4 5.5V18c0 1.7 3.6 3 8 3s8-1.3 8-3V5.5" />
    <path d="M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3" />
  </svg>
);
const TableIcon = (
  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round">
    <rect x="3" y="4" width="18" height="16" rx="2" />
    <path d="M3 10h18M10 10v10" />
  </svg>
);

/* SQL keyword 上色（CodeMirror 樣式的近似：keyword 藍、字串/數字暖色） */
const sqlColored = (text: string) => {
  const parts = text.split(/(SELECT|FROM|LIMIT)/);
  return parts.map((p, i) =>
    ["SELECT", "FROM", "LIMIT"].includes(p) ? (
      <span key={i} style={{ color: t.termBlue }}>{p}</span>
    ) : (
      <span key={i}>{p}</span>
    ),
  );
};

export const RemoteDb: React.FC<{ lang: Lang }> = ({ lang }) => {
  const frame = useCurrentFrame();
  const { durationInFrames } = useVideoConfig();

  const T_TYPE = 18;
  const T_RUN = T_TYPE + Math.ceil(QUERY.length / 0.8) + 12;
  const T_HEAD = T_RUN + 12;
  const T_ROWS = T_HEAD + 8;
  const T_FOOT = T_ROWS + 34;
  const T_HOVER = T_FOOT + 26;

  const queryDone = typedDone(frame, QUERY, T_TYPE, 0.8);
  const runDip = interpolate(frame, [T_RUN, T_RUN + 3, T_RUN + 8], [1, 0.93, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  const contentFade = interpolate(
    frame,
    [durationInFrames - 16, durationInFrames - 4],
    [1, 0],
    { extrapolateLeft: "clamp", extrapolateRight: "clamp" },
  );

  const sidebar = (
    <div style={{ display: "flex", flexDirection: "column", gap: 2 }}>
      <SideRow icon={DbIcon} label="staging" sub="postgres" active />
      <SideRow icon={TableIcon} label="orders" indent={1} active={false} />
      <SideRow icon={TableIcon} label="users" indent={1} />
      <SideRow icon={TableIcon} label="sessions" indent={1} />
    </div>
  );

  const main = (
    <>
      {/* SQL editor 區（CodeMirror：mono 13px） */}
      <div
        style={{
          height: 120,
          padding: "14px 16px",
          fontFamily: fonts.mono,
          fontSize: 13,
          lineHeight: 1.7,
          color: t.ink1,
          whiteSpace: "pre",
        }}
      >
        <span style={{ color: t.ink4, marginRight: 14 }}>1</span>
        {sqlColored(typed(frame, QUERY, T_TYPE, 0.8))}
        {queryDone ? null : <Cursor color={t.ink1} width={2} height={15} />}
      </div>

      {/* toolbar（h40, Run 按鈕 lime） */}
      <div
        style={{
          height: 40,
          display: "flex",
          alignItems: "center",
          gap: 8,
          padding: "0 10px",
          borderTop: `1px solid ${t.line1}`,
          borderBottom: `1px solid ${t.line1}`,
        }}
      >
        <span
          style={{
            height: 26,
            padding: "0 10px",
            borderRadius: 8,
            background: t.accent,
            color: "#fff",
            fontSize: 12,
            fontWeight: 500,
            display: "inline-flex",
            alignItems: "center",
            gap: 6,
            scale: String(runDip),
          }}
        >
          <svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor">
            <path d="M7 4l13 8-13 8z" />
          </svg>
          Run
          <span
            style={{
              background: "rgba(255,255,255,0.2)",
              borderRadius: 5,
              padding: "1px 5px",
              fontFamily: fonts.mono,
              fontSize: 10,
            }}
          >
            ⌘↵
          </span>
        </span>
      </div>

      {/* 結果表（mono 12px, 表頭 sticky paper-1） */}
      <div style={{ flex: 1, minHeight: 0, opacity: contentFade }}>
        {frame >= T_HEAD ? (
          <table
            style={{
              width: "100%",
              borderCollapse: "collapse",
              fontFamily: fonts.mono,
              fontSize: 12,
            }}
          >
            <thead>
              <tr style={{ background: t.paper1 }}>
                {COLS.map((cname, i) => (
                  <th
                    key={cname}
                    style={{
                      width: COL_W[i],
                      textAlign: "left",
                      padding: "10px 10px",
                      fontWeight: 600,
                      color: t.ink2,
                      borderBottom: `1px solid ${t.line1}`,
                      borderRight: `1px solid rgba(27, 26, 23, 0.06)`,
                      whiteSpace: "nowrap",
                      opacity: interpolate(frame, [T_HEAD, T_HEAD + 8], [0, 1], {
                        extrapolateLeft: "clamp",
                        extrapolateRight: "clamp",
                        easing: Easing.bezier(...EASE),
                      }),
                    }}
                  >
                    {cname}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {ROWS.map((row, r) =>
                frame >= T_ROWS + r * 7 ? (
                  <Row key={r} row={row} from={T_ROWS + r * 7} hover={r === 1 && frame >= T_HOVER} />
                ) : null,
              )}
            </tbody>
          </table>
        ) : null}
        {/* footer bar */}
        {frame >= T_FOOT ? (
          <div
            style={{
              display: "flex",
              gap: 8,
              padding: "5px 10px",
              borderTop: `1px solid ${t.line1}`,
              fontSize: 11,
              color: t.ink3,
              fontFamily: fonts.mono,
              opacity: interpolate(frame, [T_FOOT, T_FOOT + 8], [0, 1], {
                extrapolateLeft: "clamp",
                extrapolateRight: "clamp",
              }),
            }}
          >
            3 rows
          </div>
        ) : null}
      </div>
    </>
  );

  return (
    <Workbench
      mode={2}
      sectionLabel="DATABASE"
      sidebar={sidebar}
      sidebarFooter={
        <DashedButton label={lang === "zh" ? "新增連線…" : "New connection…"} />
      }
      main={main}
      lang={lang}
    />
  );
};

const Row: React.FC<{ row: string[]; from: number; hover: boolean }> = ({
  row,
  from,
  hover,
}) => {
  const rowIn = useRowIn(from);
  return (
    <tr
      style={{
        background: hover ? t.hover : "transparent",
        opacity: rowIn.opacity,
        translate: rowIn.translate,
      }}
    >
      {row.map((cell, i) => (
        <td
          key={i}
          style={{
            padding: "4px 10px",
            color: t.ink1,
            borderBottom: `1px solid rgba(27, 26, 23, 0.06)`,
            borderRight: `1px solid rgba(27, 26, 23, 0.06)`,
            whiteSpace: "nowrap",
          }}
        >
          {cell}
        </td>
      ))}
    </tr>
  );
};
