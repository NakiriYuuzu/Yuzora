// Tokens 1:1 對齊 app 本體 src/styles.css（light theme）
import { loadFont as loadHanken } from "@remotion/google-fonts/HankenGrotesk";
import { loadFont as loadMono } from "@remotion/google-fonts/JetBrainsMono";
import { loadFont as loadNotoTC } from "@remotion/google-fonts/NotoSansTC";
import { loadFont as loadSerif } from "@remotion/google-fonts/Newsreader";

const hanken = loadHanken();
const mono = loadMono();
const notoTC = loadNotoTC();
const serif = loadSerif();

export const fonts = {
  sans: `${hanken.fontFamily}, ${notoTC.fontFamily}, sans-serif`,
  mono: `${mono.fontFamily}, monospace`,
  serif: `${serif.fontFamily}, Georgia, serif`,
};

export const t = {
  // paper / ink / line
  paper0: "#ffffff",
  paper1: "#fbfaf6",
  paper2: "#f4f1ea",
  paper3: "#ebe7dd",
  ink0: "#1b1a17",
  ink1: "#2e2c28",
  ink2: "#57534b",
  ink3: "#8a857a",
  ink4: "#b6b0a3",
  line1: "rgba(27, 26, 23, 0.10)",
  line2: "rgba(27, 26, 23, 0.16)",
  // accent
  accent: "#86b81f",
  accentInk: "#5f8c1e",
  accentRgb: "134, 184, 31",
  // surfaces / glass
  bg: `radial-gradient(125% 120% at 0% 0%, #e9f4c8 0%, transparent 46%),
       radial-gradient(120% 115% at 100% 2%, #cdeed3 0%, transparent 50%),
       radial-gradient(125% 130% at 96% 100%, #c4dbf6 0%, transparent 54%),
       radial-gradient(120% 120% at 8% 100%, #f6e6c8 0%, transparent 48%),
       #f6f5ef`,
  glass: "rgba(251, 250, 246, 0.55)",
  glassStrong: "rgba(251, 250, 246, 0.7)",
  field: "rgba(255, 255, 255, 0.62)",
  panel: "rgba(255, 255, 255, 0.55)",
  active: "rgba(255, 255, 255, 0.78)",
  hover: "rgba(255, 255, 255, 0.45)",
  sunk: "rgba(244, 241, 234, 0.5)",
  solid: "#ffffff",
  // terminal
  termBg: "#f1ede4",
  termBar: "#e8e2d7",
  termFg: "#46433c",
  termFg2: "#8a8691",
  termLine: "rgba(27, 26, 23, 0.08)",
  termGreen: "#2f8f5f",
  termBlue: "#2456cc",
  termLime: "#5f8c1e",
  termCoral: "#c0562f",
  termOk: "#1f8a5b",
  termAmber: "#a8690f",
  // soft tints
  mintSoft: "#e0f6ee",
  amberSoft: "#fff2dc",
  dangerSoft: "#ffe6e9",
  // thread kind styles（AgentZonePanel THREAD_KIND_STYLE）
  diffFg: "#0f7a55",
  diffBd: "rgba(43, 191, 138, 0.28)",
  permFg: "#9a6512",
  permBd: "rgba(214, 138, 12, 0.3)",
  acpPurple: "#5b3fd1",
  // gradients
  gradSunrise: "linear-gradient(160deg, #ffb23e 0%, #ff6b54 55%, #e0539b 100%)",
  gradDusk: "linear-gradient(160deg, #7b5bff 0%, #2f6bff 100%)",
  // shadows
  shadowXs: "0 1px 2px rgba(27, 26, 23, 0.06)",
  shadowSm: "0 2px 6px rgba(27, 26, 23, 0.07), 0 1px 2px rgba(27, 26, 23, 0.05)",
  shadowLg: "0 16px 40px rgba(27, 26, 23, 0.12), 0 4px 12px rgba(27, 26, 23, 0.07)",
};

export const EASE: [number, number, number, number] = [0.22, 1, 0.36, 1];
export const SPRING: [number, number, number, number] = [0.34, 1.56, 0.64, 1];
