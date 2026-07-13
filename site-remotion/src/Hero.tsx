import React from "react";
import {
  AbsoluteFill,
  Easing,
  Sequence,
  interpolate,
  useCurrentFrame,
} from "remotion";
import { AgentZone } from "./AgentZone";
import { RemoteDb } from "./RemoteDb";
import { TerminalGit } from "./TerminalGit";
import { EASE, SPRING, fonts, t } from "./theme";
import { useRise } from "./ui";

type Lang = "zh" | "en";

const COPY: Record<Lang, Record<string, string>> = {
  zh: {
    badge: "開源 · Tauri 打造",
    tagline1: "把 agent、遠端與資料",
    tagline2: "收進同一張桌面。",
    s1kicker: "01 · AgentZone",
    s1title: "ACP agent 並肩工作",
    s2kicker: "02 · SSH ＆ 資料庫",
    s2title: "遠端即在地",
    s3kicker: "03 · Terminal ＆ Git",
    s3title: "除錯不離開工作台",
    slogan: "把整個開發日常，收進一張桌面。",
    slogan2: "夕空下的開發工作台",
    endNote: "macOS · Windows · Linux",
  },
  en: {
    badge: "Open source · Built with Tauri",
    tagline1: "Agents, remotes and data —",
    tagline2: "all on one desk.",
    s1kicker: "01 · AgentZone",
    s1title: "Work alongside ACP agents",
    s2kicker: "02 · SSH & databases",
    s2title: "Remote feels local",
    s3kicker: "03 · Terminal & git",
    s3title: "Debug without leaving the bench",
    slogan: "Your whole dev day, on one desk.",
    slogan2: "a dev workbench under the evening sky",
    endNote: "macOS · Windows · Linux",
  },
};

/* ---------- timeline（30fps，共 555 frames = 18.5s） ---------- */
const T = {
  title: { start: 0, end: 90 },
  s1: { start: 82, end: 212, inner: 120 }, // AgentZone 精華：diff + 權限確認
  s2: { start: 204, end: 334, inner: 70 }, // RemoteDb 精華：query + rows
  s3: { start: 326, end: 456, inner: 80 }, // TerminalGit 精華：log + cherry-pick
  end: { start: 448, end: 555 },
};
export const HERO_DURATION = T.end.end;

const FADE = 12;

/* 場景通用淡入淡出（相對本 Sequence 的 local frame） */
const useSceneFade = (duration: number) => {
  const frame = useCurrentFrame();
  return interpolate(
    frame,
    [0, FADE, duration - FADE, duration],
    [0, 1, 1, 0],
    {
      extrapolateLeft: "clamp",
      extrapolateRight: "clamp",
      easing: Easing.bezier(...EASE),
    },
  );
};

/* ---------- 品牌 logo（sunrise 漸層方塊 Y） ---------- */
const Logo: React.FC<{ size: number }> = ({ size }) => (
  <div
    style={{
      width: size,
      height: size,
      borderRadius: size * 0.3,
      backgroundImage: t.gradSunrise,
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
      color: "#fff",
      fontWeight: 700,
      fontSize: size * 0.52,
      fontFamily: fonts.sans,
      boxShadow: t.shadowLg,
    }}
  >
    Y
  </div>
);

/* ---------- 片頭品牌卡 ---------- */
const TitleCard: React.FC<{ lang: Lang }> = ({ lang }) => {
  const frame = useCurrentFrame();
  const c = COPY[lang];
  const fade = useSceneFade(T.title.end - T.title.start);

  const logoScale = interpolate(frame, [4, 22], [0.6, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: Easing.bezier(...SPRING),
  });
  const logoOpacity = interpolate(frame, [4, 14], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  const badge = useRise(18, 12, 8);
  const line1 = useRise(26, 14, 14);
  const line2 = useRise(34, 14, 14);
  // 片尾微放大帶入下一景
  const drift = interpolate(frame, [0, T.title.end], [1, 1.035], {
    easing: Easing.bezier(0.4, 0, 1, 1),
  });

  return (
    <AbsoluteFill
      style={{
        background: t.bg,
        opacity: fade,
        alignItems: "center",
        justifyContent: "center",
        fontFamily: fonts.sans,
      }}
    >
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          gap: 34,
          scale: String(drift),
        }}
      >
        <div style={{ scale: String(logoScale), opacity: logoOpacity }}>
          <Logo size={108} />
        </div>
        <div
          style={{
            fontFamily: fonts.mono,
            fontSize: 22,
            fontWeight: 600,
            color: t.accentInk,
            background: `rgba(${t.accentRgb}, 0.14)`,
            borderRadius: 12,
            padding: "8px 20px",
            opacity: badge.opacity,
            translate: badge.translate,
          }}
        >
          {c.badge}
        </div>
        <div style={{ textAlign: "center", lineHeight: 1.28 }}>
          <div
            style={{
              fontSize: 76,
              fontWeight: 800,
              letterSpacing: "-0.01em",
              color: t.ink0,
              opacity: line1.opacity,
              translate: line1.translate,
            }}
          >
            {c.tagline1}
          </div>
          <div
            style={{
              fontSize: 76,
              fontWeight: 800,
              letterSpacing: "-0.01em",
              color: t.ink0,
              opacity: line2.opacity,
              translate: line2.translate,
            }}
          >
            {lang === "zh" ? (
              <>
                收進<em style={{ fontStyle: "normal", color: t.accentInk }}>同一張桌面</em>。
              </>
            ) : (
              <>
                all on <em style={{ fontStyle: "normal", color: t.accentInk }}>one desk</em>.
              </>
            )}
          </div>
        </div>
      </div>
    </AbsoluteFill>
  );
};

/* ---------- 功能場景：內嵌既有 composition 精華片段＋左下標籤 ---------- */
const FeatureScene: React.FC<{
  duration: number;
  innerFrom: number;
  kicker: string;
  title: string;
  children: React.ReactNode;
}> = ({ duration, innerFrom, kicker, title, children }) => {
  const fade = useSceneFade(duration);
  const chip = useRise(FADE + 2, 12, 12);
  return (
    <AbsoluteFill style={{ opacity: fade }}>
      {/* 內層 Sequence 負偏移＝從精華段落中途播放 */}
      <Sequence from={-innerFrom}>{children}</Sequence>
      <div
        style={{
          position: "absolute",
          left: 28,
          bottom: 52,
          display: "flex",
          alignItems: "center",
          gap: 16,
          padding: "14px 24px",
          borderRadius: 16,
          background: t.glassStrong,
          border: `1px solid ${t.line2}`,
          boxShadow: t.shadowLg,
          backdropFilter: "blur(20px)",
          opacity: chip.opacity,
          translate: chip.translate,
        }}
      >
        <span
          style={{
            fontFamily: fonts.mono,
            fontSize: 21,
            fontWeight: 600,
            color: t.accentInk,
          }}
        >
          {kicker}
        </span>
        <span
          style={{
            fontFamily: fonts.sans,
            fontSize: 24,
            fontWeight: 700,
            letterSpacing: "-0.01em",
            color: t.ink0,
          }}
        >
          {title}
        </span>
      </div>
    </AbsoluteFill>
  );
};

/* ---------- 片尾 slogan 卡 ---------- */
const EndCard: React.FC<{ lang: Lang }> = ({ lang }) => {
  const frame = useCurrentFrame();
  const c = COPY[lang];
  const opacity = interpolate(frame, [0, FADE], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: Easing.bezier(...EASE),
  });
  const logoScale = interpolate(frame, [6, 26], [0.65, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: Easing.bezier(...SPRING),
  });
  const word = useRise(16, 14, 12);
  const slogan = useRise(28, 14, 12);
  const slogan2 = useRise(40, 14, 10);
  const note = useRise(52, 12, 10);

  return (
    <AbsoluteFill
      style={{
        background: t.bg,
        opacity,
        alignItems: "center",
        justifyContent: "center",
      }}
    >
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          gap: 30,
        }}
      >
        <div style={{ scale: String(logoScale) }}>
          <Logo size={100} />
        </div>
        <div
          style={{
            fontFamily: fonts.serif,
            fontSize: 92,
            fontWeight: 600,
            letterSpacing: "-0.01em",
            color: t.ink0,
            lineHeight: 1.1,
            opacity: word.opacity,
            translate: word.translate,
          }}
        >
          Yuzora
        </div>
        <div
          style={{
            fontFamily: fonts.sans,
            fontSize: 38,
            fontWeight: 650,
            letterSpacing: "-0.01em",
            color: t.ink1,
            opacity: slogan.opacity,
            translate: slogan.translate,
          }}
        >
          {c.slogan}
        </div>
        <div
          style={{
            fontFamily: fonts.mono,
            fontSize: 21,
            color: t.ink3,
            marginTop: -8,
            opacity: slogan2.opacity,
            translate: slogan2.translate,
          }}
        >
          {c.slogan2}
        </div>
        <div
          style={{
            fontFamily: fonts.mono,
            fontSize: 21,
            color: t.ink3,
            display: "flex",
            alignItems: "center",
            gap: 14,
            opacity: note.opacity,
            translate: note.translate,
          }}
        >
          <span
            style={{
              width: 8,
              height: 8,
              borderRadius: "50%",
              background: t.accent,
              display: "inline-block",
            }}
          />
          {c.endNote}
        </div>
      </div>
    </AbsoluteFill>
  );
};

/* ---------- Hero 主 composition ---------- */
export const Hero: React.FC<{ lang: Lang }> = ({ lang }) => {
  const c = COPY[lang];
  return (
    <AbsoluteFill style={{ background: t.bg }}>
      <Sequence from={T.s1.start} durationInFrames={T.s1.end - T.s1.start}>
        <FeatureScene
          duration={T.s1.end - T.s1.start}
          innerFrom={T.s1.inner}
          kicker={c.s1kicker}
          title={c.s1title}
        >
          <AgentZone lang={lang} />
        </FeatureScene>
      </Sequence>
      <Sequence from={T.s2.start} durationInFrames={T.s2.end - T.s2.start}>
        <FeatureScene
          duration={T.s2.end - T.s2.start}
          innerFrom={T.s2.inner}
          kicker={c.s2kicker}
          title={c.s2title}
        >
          <RemoteDb lang={lang} />
        </FeatureScene>
      </Sequence>
      <Sequence from={T.s3.start} durationInFrames={T.s3.end - T.s3.start}>
        <FeatureScene
          duration={T.s3.end - T.s3.start}
          innerFrom={T.s3.inner}
          kicker={c.s3kicker}
          title={c.s3title}
        >
          <TerminalGit lang={lang} />
        </FeatureScene>
      </Sequence>
      <Sequence durationInFrames={T.title.end}>
        <TitleCard lang={lang} />
      </Sequence>
      <Sequence from={T.end.start}>
        <EndCard lang={lang} />
      </Sequence>
    </AbsoluteFill>
  );
};
