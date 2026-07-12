import React from "react";
import { AbsoluteFill } from "remotion";
import { fonts, t } from "./theme";

/* ---------- 極簡 lucide 風 icon（stroke 1.5, 16px viewBox 24） ---------- */
const I: React.FC<{ d: React.ReactNode; size?: number; color?: string }> = ({
  d,
  size = 16,
  color = "currentColor",
}) => (
  <svg
    width={size}
    height={size}
    viewBox="0 0 24 24"
    fill="none"
    stroke={color}
    strokeWidth={1.8}
    strokeLinecap="round"
    strokeLinejoin="round"
  >
    {d}
  </svg>
);

export const icons = {
  files: (
    <>
      <rect x="8" y="3" width="12" height="15" rx="2" />
      <path d="M4 7v12a2 2 0 0 0 2 2h10" />
    </>
  ),
  git: (
    <>
      <circle cx="6" cy="6" r="2.6" />
      <circle cx="18" cy="18" r="2.6" />
      <path d="M6 9v3a4 4 0 0 0 4 4h5" />
    </>
  ),
  db: (
    <>
      <ellipse cx="12" cy="5.5" rx="8" ry="3" />
      <path d="M4 5.5V18c0 1.7 3.6 3 8 3s8-1.3 8-3V5.5" />
      <path d="M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3" />
    </>
  ),
  ssh: (
    <>
      <rect x="3" y="4" width="18" height="16" rx="3" />
      <path d="M7 9l3 3-3 3M12 15h5" />
    </>
  ),
  bot: (
    <>
      <rect x="5" y="8" width="14" height="11" rx="3" />
      <path d="M12 8V4M8.5 13h.01M15.5 13h.01" />
    </>
  ),
  sidebar: (
    <>
      <rect x="3" y="4" width="18" height="16" rx="3" />
      <path d="M9.5 4v16" />
    </>
  ),
  terminal: (
    <>
      <path d="M4 17l4-5-4-5M11 17h9" />
    </>
  ),
  preview: (
    <>
      <rect x="3" y="4" width="18" height="16" rx="3" />
      <path d="M3 9h18" />
    </>
  ),
  search: (
    <>
      <circle cx="11" cy="11" r="6.5" />
      <path d="M20 20l-3.5-3.5" />
    </>
  ),
  branch: (
    <>
      <circle cx="6" cy="5" r="2.4" />
      <circle cx="6" cy="19" r="2.4" />
      <circle cx="18" cy="9" r="2.4" />
      <path d="M6 7.5v9M18 11.5a7 7 0 0 1-7 5.5H6" />
    </>
  ),
};

/* ---------- 左側 activity rail（WorkspaceRail：w60, 按鈕 38×32 r10） ---------- */
const Rail: React.FC = () => (
  <div
    style={{
      width: 60,
      display: "flex",
      flexDirection: "column",
      alignItems: "center",
      gap: 5,
      paddingTop: 13,
      paddingBottom: 11,
    }}
  >
    {[icons.sidebar, icons.terminal, icons.preview].map((ic, i) => (
      <div
        key={i}
        style={{
          width: 38,
          height: 32,
          borderRadius: 10,
          background: t.solid,
          border: `1px solid ${t.line1}`,
          boxShadow: t.shadowXs,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          color: i === 0 ? t.accentInk : t.ink3,
        }}
      >
        <I d={ic} size={15} />
      </div>
    ))}
    <div style={{ flex: 1 }} />
    <div
      style={{
        width: 38,
        height: 38,
        borderRadius: 11,
        border: `1.5px dashed ${t.line2}`,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        color: t.ink3,
        fontSize: 16,
      }}
    >
      +
    </div>
    <div
      style={{
        width: 32,
        height: 32,
        borderRadius: "50%",
        backgroundImage: t.gradDusk,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        color: "#fff",
        fontSize: 13,
        fontWeight: 600,
        marginTop: 6,
      }}
    >
      Y
    </div>
  </div>
);

/* ---------- sidebar 模式 tab 列 ---------- */
const ModeTabs: React.FC<{ active: number }> = ({ active }) => (
  <div style={{ display: "flex", gap: 8, padding: "12px 2px 10px" }}>
    {[icons.files, icons.git, icons.db, icons.ssh, icons.bot].map((ic, i) => (
      <div
        key={i}
        style={{
          width: 40,
          height: 34,
          borderRadius: 10,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          background: i === active ? t.solid : "transparent",
          boxShadow: i === active ? t.shadowSm : "none",
          color: i === active ? t.accentInk : t.ink3,
        }}
      >
        <I d={ic} size={16} />
      </div>
    ))}
  </div>
);

const WB_COPY = {
  zh: { search: "搜尋或執行命令" },
  en: { search: "Search or run a command" },
};

/* ---------- 完整 workbench 外框 ---------- */
export const Workbench: React.FC<{
  mode: number; // 0 files / 1 git / 2 db / 3 ssh / 4 agent
  sectionLabel: string;
  sidebar: React.ReactNode;
  sidebarFooter?: React.ReactNode;
  main: React.ReactNode;
  drawer?: React.ReactNode;
  statusRight?: string;
  lang?: "zh" | "en";
}> = ({
  mode,
  sectionLabel,
  sidebar,
  sidebarFooter,
  main,
  drawer,
  statusRight,
  lang = "zh",
}) => {
  return (
    <AbsoluteFill
      style={{
        background: t.bg,
        fontFamily: fonts.sans,
        fontSize: 13,
        color: t.ink1,
      }}
    >
      <div style={{ display: "flex", flex: 1, minHeight: 0 }}>
        <Rail />
        {/* sidebar glass 卡片 */}
        <div
          style={{
            width: 265,
            margin: "8px 0",
            borderRadius: 20,
            background: t.glass,
            border: `1px solid ${t.line1}`,
            display: "flex",
            flexDirection: "column",
            padding: "16px 14px 14px",
            minHeight: 0,
          }}
        >
          {/* header：logo + serif 標題 + 路徑 */}
          <div style={{ display: "flex", gap: 11, alignItems: "center" }}>
            <div
              style={{
                width: 28,
                height: 28,
                borderRadius: 9,
                backgroundImage: t.gradSunrise,
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                color: "#fff",
                fontSize: 13,
                fontWeight: 700,
              }}
            >
              Y
            </div>
            <div>
              <div
                style={{
                  fontFamily: fonts.serif,
                  fontSize: 19,
                  fontWeight: 600,
                  letterSpacing: "-0.01em",
                  color: t.ink0,
                  lineHeight: 1.15,
                }}
              >
                Yuzora
              </div>
              <div style={{ fontFamily: fonts.mono, fontSize: 10.5, color: t.ink3 }}>
                ~/App/Tauri/yuzora
              </div>
            </div>
          </div>
          {/* 搜尋框 */}
          <div
            style={{
              marginTop: 13,
              height: 32,
              borderRadius: 10,
              background: t.field,
              border: `1px solid ${t.line1}`,
              boxShadow: t.shadowXs,
              display: "flex",
              alignItems: "center",
              gap: 8,
              padding: "0 6px 0 10px",
              color: t.ink3,
              fontSize: 12,
            }}
          >
            <I d={icons.search} size={13} />
            <span style={{ flex: 1 }}>{WB_COPY[lang].search}</span>
            <span
              style={{
                fontFamily: fonts.mono,
                fontSize: 10,
                border: `1px solid ${t.line1}`,
                borderRadius: 5,
                padding: "1px 5px",
                background: t.solid,
              }}
            >
              ⌘K
            </span>
          </div>
          <ModeTabs active={mode} />
          <div
            style={{
              fontSize: 11,
              fontWeight: 600,
              letterSpacing: "0.08em",
              color: t.ink3,
              padding: "2px 4px 8px",
            }}
          >
            {sectionLabel}
          </div>
          <div style={{ flex: 1, minHeight: 0, overflow: "hidden" }}>{sidebar}</div>
          {sidebarFooter}
        </div>
        {/* 主區：panel 卡片（AppShell padding 8 / gap 10） */}
        <div
          style={{
            flex: 1,
            minWidth: 0,
            display: "flex",
            flexDirection: "column",
            gap: 10,
            padding: 8,
            paddingLeft: 14,
          }}
        >
          <div
            style={{
              flex: 1,
              minHeight: 0,
              borderRadius: 20,
              background: t.paper0,
              border: `1px solid ${t.line1}`,
              boxShadow: t.shadowLg,
              overflow: "hidden",
              display: "flex",
              flexDirection: "column",
            }}
          >
            {main}
          </div>
          {drawer}
        </div>
      </div>
      {/* status bar */}
      <div
        style={{
          height: 30,
          borderTop: `1px solid ${t.line1}`,
          background: t.glassStrong,
          display: "flex",
          alignItems: "center",
          gap: 10,
          padding: "0 12px",
          fontFamily: fonts.mono,
          fontSize: 11.5,
          color: t.ink2,
        }}
      >
        <span style={{ color: t.ink1, fontWeight: 500 }}>Yuzora</span>
        <span style={{ display: "flex", alignItems: "center", gap: 6 }}>
          <span
            style={{
              width: 7,
              height: 7,
              borderRadius: "50%",
              background: t.accent,
            }}
          />
          <I d={icons.branch} size={12} />
          main
        </span>
        <div style={{ flex: 1 }} />
        <span style={{ color: t.ink3 }}>{statusRight ?? ""}</span>
      </div>
    </AbsoluteFill>
  );
};

/* ---------- sidebar 通用列（h30 r8） ---------- */
export const SideRow: React.FC<{
  icon?: React.ReactNode;
  label: string;
  sub?: string;
  active?: boolean;
  badge?: React.ReactNode;
  indent?: number;
  mono?: boolean;
}> = ({ icon, label, sub, active = false, badge, indent = 0, mono = false }) => (
  <div
    style={{
      height: 30,
      borderRadius: 8,
      display: "flex",
      alignItems: "center",
      gap: 9,
      padding: "0 8px",
      paddingLeft: 8 + indent * 15,
      background: active ? t.active : "transparent",
      boxShadow: active ? t.shadowXs : "none",
      color: active ? t.ink0 : t.ink2,
    }}
  >
    {icon ? (
      <span style={{ color: active ? t.accentInk : t.ink3, display: "flex" }}>{icon}</span>
    ) : null}
    <span
      style={{
        fontSize: 12.5,
        fontWeight: active ? 600 : 400,
        fontFamily: mono ? fonts.mono : fonts.sans,
        whiteSpace: "nowrap",
        overflow: "hidden",
        textOverflow: "ellipsis",
        flex: 1,
      }}
    >
      {label}
      {sub ? (
        <span style={{ fontSize: 10.5, color: t.ink4, marginLeft: 6 }}>{sub}</span>
      ) : null}
    </span>
    {badge}
  </div>
);

/* ---------- sidebar 底部虛線按鈕 ---------- */
export const DashedButton: React.FC<{ label: string }> = ({ label }) => (
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
      marginTop: 10,
    }}
  >
    <span style={{ fontSize: 14 }}>+</span> {label}
  </div>
);
