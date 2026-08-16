import React from "react";
import { Easing, interpolate, useCurrentFrame, useVideoConfig } from "remotion";
import { EASE, fonts, t } from "./theme";
import { PulseDot, useRise } from "./ui";
import { Workbench } from "./Workbench";

type Lang = "zh" | "en";

type Copy = {
  section: string;
  attention: string;
  agents: string;
  newTerminal: string;
  controller: string;
  terminalTitle: string;
  attentionTitle: string;
  attentionMeta: string;
  sessionsLabel: string;
  sessionDefault: string;
  sessionReview: string;
};

const COPY: Record<Lang, Copy> = {
  zh: {
    section: "ADE · HERDR",
    attention: "ATTENTION",
    agents: "AGENTS",
    newTerminal: "新增 HERDR terminal",
    controller: "controller",
    terminalTitle: "HERDR · build-check",
    attentionTitle: "docs-review 等待處理",
    attentionMeta: "blocked · Docs",
    sessionsLabel: "NAMED SESSIONS",
    sessionDefault: "default",
    sessionReview: "review",
  },
  en: {
    section: "ADE · HERDR",
    attention: "ATTENTION",
    agents: "AGENTS",
    newTerminal: "New HERDR terminal",
    controller: "controller",
    terminalTitle: "HERDR · build-check",
    attentionTitle: "docs-review needs attention",
    attentionMeta: "blocked · Docs",
    sessionsLabel: "NAMED SESSIONS",
    sessionDefault: "default",
    sessionReview: "review",
  },
};

const StatusDot: React.FC<{ color: string; pulse?: boolean }> = ({ color, pulse }) =>
  pulse ? (
    <PulseDot color={color} size={7} />
  ) : (
    <span
      style={{
        width: 7,
        height: 7,
        borderRadius: "50%",
        background: color,
        display: "inline-block",
        flexShrink: 0,
      }}
    />
  );

const SessionPills: React.FC<{ copy: Copy }> = ({ copy }) => {
  const rise = useRise(8, 12, 8);
  return (
    <div
      style={{
        opacity: rise.opacity,
        translate: rise.translate,
        display: "flex",
        flexWrap: "wrap",
        gap: 6,
        padding: "0 2px 10px",
      }}
    >
      {[copy.sessionDefault, copy.sessionReview].map((name, index) => (
        <div
          key={name}
          style={{
            borderRadius: 999,
            border: `1px solid ${index === 0 ? `rgba(${t.accentRgb}, 0.45)` : t.line2}`,
            background: index === 0 ? t.active : "transparent",
            color: index === 0 ? t.ink0 : t.ink3,
            padding: "4px 10px",
            fontSize: 11,
            fontWeight: 600,
            lineHeight: 1.2,
          }}
        >
          {name}
        </div>
      ))}
    </div>
  );
};

const SectionLabel: React.FC<{ children: React.ReactNode; from: number }> = ({
  children,
  from,
}) => {
  const rise = useRise(from, 10, 6);
  return (
    <div
      style={{
        opacity: rise.opacity,
        translate: rise.translate,
        padding: "5px 8px 4px",
        fontSize: 10,
        fontWeight: 700,
        letterSpacing: "0.09em",
        color: t.ink4,
      }}
    >
      {children}
    </div>
  );
};

const AgentRow: React.FC<{
  from: number;
  name: string;
  space: string;
  status: string;
  color: string;
  active?: boolean;
  pulse?: boolean;
}> = ({ from, name, space, status, color, active, pulse }) => {
  const rise = useRise(from, 10, 7);
  return (
    <div
      style={{
        opacity: rise.opacity,
        translate: rise.translate,
        display: "flex",
        alignItems: "center",
        gap: 8,
        minHeight: 42,
        borderRadius: 10,
        padding: "7px 8px",
        background: active ? t.active : "transparent",
        boxShadow: active ? t.shadowXs : "none",
      }}
    >
      <StatusDot color={color} pulse={pulse} />
      <div style={{ minWidth: 0, flex: 1 }}>
        <div
          style={{
            color: t.ink1,
            fontSize: 12.5,
            fontWeight: 600,
            whiteSpace: "nowrap",
            overflow: "hidden",
            textOverflow: "ellipsis",
          }}
        >
          {name}
        </div>
        <div style={{ color: t.ink4, fontSize: 10 }}>{space}</div>
      </div>
      <span style={{ color: t.ink4, fontFamily: fonts.mono, fontSize: 9.5 }}>
        {status}
      </span>
    </div>
  );
};

const Sidebar: React.FC<{ lang: Lang }> = ({ lang }) => {
  const copy = COPY[lang];
  const attention = useRise(30, 12, 8);
  return (
    <div style={{ display: "flex", height: "100%", flexDirection: "column" }}>
      <div
        style={{
          padding: "0 8px 4px",
          color: t.ink4,
          fontSize: 9.5,
          fontWeight: 700,
          letterSpacing: "0.09em",
        }}
      >
        {copy.sessionsLabel}
      </div>
      <SessionPills copy={copy} />
      <SectionLabel from={22}>{copy.attention}</SectionLabel>
      <div
        style={{
          opacity: attention.opacity,
          translate: attention.translate,
          display: "flex",
          alignItems: "center",
          gap: 8,
          marginBottom: 5,
          minHeight: 43,
          borderRadius: 10,
          padding: "7px 8px",
          background: t.amberSoft,
          border: `1px solid ${t.permBd}`,
        }}
      >
        <StatusDot color={t.termAmber} pulse />
        <div style={{ minWidth: 0, flex: 1 }}>
          <div style={{ color: t.ink1, fontSize: 12, fontWeight: 600 }}>
            {copy.attentionTitle}
          </div>
          <div style={{ color: t.ink4, fontSize: 10 }}>{copy.attentionMeta}</div>
        </div>
      </div>
      <SectionLabel from={38}>{copy.agents}</SectionLabel>
      <AgentRow
        from={46}
        name="build-check"
        space="Yuzora"
        status="working"
        color={t.accent}
        pulse
        active
      />
      <AgentRow
        from={55}
        name="docs-review"
        space="Docs"
        status="blocked"
        color="#ffb23e"
      />
      <AgentRow
        from={64}
        name="release-notes"
        space="Herdr"
        status="done"
        color={t.termOk}
      />
      <div style={{ flex: 1 }} />
      <div
        style={{
          height: 34,
          borderRadius: 10,
          border: `1.5px dashed ${t.line2}`,
          color: t.ink3,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          gap: 6,
          fontSize: 12.5,
        }}
      >
        <span style={{ fontSize: 15 }}>+</span>
        {copy.newTerminal}
      </div>
    </div>
  );
};

const TerminalPane: React.FC<{
  title: string;
  focused?: boolean;
  children: React.ReactNode;
}> = ({ title, focused, children }) => (
  <div
    style={{
      display: "flex",
      minWidth: 0,
      minHeight: 0,
      flex: 1,
      flexDirection: "column",
      background: t.termBg,
      border: focused ? `2px solid rgba(${t.accentRgb}, 0.55)` : `1px solid ${t.termLine}`,
    }}
  >
    <div
      style={{
        height: 28,
        display: "flex",
        alignItems: "center",
        gap: 7,
        padding: "0 9px",
        background: t.termBar,
        borderBottom: `1px solid ${t.termLine}`,
        color: t.termFg2,
        fontFamily: fonts.mono,
        fontSize: 9.5,
      }}
    >
      <StatusDot color={focused ? t.accent : t.ink4} />
      {title}
    </div>
    <pre
      style={{
        margin: 0,
        padding: "11px 12px",
        color: t.termFg,
        fontFamily: fonts.mono,
        fontSize: 10.5,
        lineHeight: 1.55,
        whiteSpace: "pre-wrap",
      }}
    >
      {children}
    </pre>
  </div>
);

const HerdrSurface: React.FC<{ lang: Lang }> = ({ lang }) => {
  const frame = useCurrentFrame();
  const copy = COPY[lang];
  const paneOpacity = interpolate(frame, [70, 88], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: Easing.bezier(...EASE),
  });
  const checkDone = frame >= 136;
  const docsLine = frame >= 116 ? "docs: terminology check\nstatus: needs attention" : "docs: terminology check";
  return (
    <>
      <div
        style={{
          height: 38,
          display: "flex",
          alignItems: "center",
          gap: 8,
          padding: "0 12px",
          borderBottom: `1px solid ${t.line1}`,
          background: t.paper0,
        }}
      >
        <div
          style={{
            height: 29,
            display: "flex",
            alignItems: "center",
            gap: 7,
            padding: "0 10px",
            borderRadius: 9,
            background: t.active,
            color: t.ink1,
            fontSize: 11.5,
            fontWeight: 600,
          }}
        >
          <StatusDot color={t.accent} pulse />
          {copy.terminalTitle}
        </div>
        <div
          style={{
            height: 22,
            display: "flex",
            alignItems: "center",
            borderRadius: 6,
            background: t.sunk,
            color: t.ink3,
            padding: "0 7px",
            fontFamily: fonts.mono,
            fontSize: 9.5,
          }}
        >
          {copy.controller}
        </div>
        <div style={{ flex: 1 }} />
        <span style={{ color: t.ink4, fontFamily: fonts.mono, fontSize: 9.5 }}>
          default · Yuzora · tab-build
        </span>
      </div>
      <div
        style={{
          opacity: paneOpacity,
          display: "flex",
          minHeight: 0,
          flex: 1,
          gap: 4,
          padding: 4,
          background: t.termBar,
        }}
      >
        <TerminalPane title="build-check · pane-a" focused>
          <span style={{ color: t.termLime }}>yuzora %</span> bun run typecheck{"\n"}
          <span style={{ color: t.termBlue }}>→</span> checking frontend{"\n"}
          {checkDone ? (
            <span style={{ color: t.termOk }}>✓ typecheck passed</span>
          ) : (
            <span style={{ color: t.termAmber }}>… resolving modules</span>
          )}
          {"\n\n"}
          <span style={{ color: t.termLime }}>yuzora %</span> bun run test src/lib/herdrPages.test.ts
        </TerminalPane>
        <div style={{ display: "flex", width: "39%", minHeight: 0, flexDirection: "column", gap: 4 }}>
          <TerminalPane title="docs-review · pane-b">
            {docsLine}
          </TerminalPane>
          <TerminalPane title="release-notes · pane-c">
            release: v0.0.1{"\n"}
            runtime: HERDR{"\n"}
            <span style={{ color: t.termOk }}>state: done</span>
          </TerminalPane>
        </div>
      </div>
    </>
  );
};

export const AdeHerdr: React.FC<{ lang: Lang }> = ({ lang }) => {
  const frame = useCurrentFrame();
  const { durationInFrames } = useVideoConfig();
  const contentOpacity = interpolate(frame, [durationInFrames - 14, durationInFrames - 3], [1, 0], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  return (
    <div style={{ width: "100%", height: "100%", opacity: contentOpacity }}>
      <Workbench
        mode={4}
        adeRail
        sectionLabel={COPY[lang].section}
        sidebar={<Sidebar lang={lang} />}
        main={<HerdrSurface lang={lang} />}
        statusRight="HERDR · default · connected"
        lang={lang}
      />
    </div>
  );
};
