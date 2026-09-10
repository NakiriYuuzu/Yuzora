import {
  AbsoluteFill,
  OffthreadVideo,
  interpolate,
  staticFile,
  useCurrentFrame,
} from "remotion";

type Feature = "ade-herdr" | "terminal-git" | "remote-db";
export type RecordedWorkbenchProps = { feature: Feature; lang: "zh" | "en" };
const labels = {
  "ade-herdr": {
    zh: ["你的工作，你的 Space。", "Agents · 終端機 · 編輯器"],
    en: ["Your work. Your Space.", "Agents · terminal · editor"],
  },
  "terminal-git": {
    zh: ["每一行變更，都看得清楚。", "Git diff · 並排比較 · 終端機"],
    en: ["Every change, in view.", "Git diff · split view · terminal"],
  },
  "remote-db": {
    zh: ["查詢資料，也照顧你的喜好。", "SQL · 查詢結果 · 主題色"],
    en: ["Explore your data. Make it yours.", "SQL · query results · themes"],
  },
};
/** The source is a recording of the real AppShell with the public demo transport. */
export const RecordedWorkbench: React.FC<RecordedWorkbenchProps> = ({
  feature,
  lang,
}) => {
  const frame = useCurrentFrame();
  const [title, detail] = labels[feature][lang];
  return (
    <AbsoluteFill
      style={{ background: "#101422", fontFamily: "system-ui, sans-serif" }}
    >
      <div
        style={{
          height: 60,
          display: "flex",
          alignItems: "center",
          padding: "0 28px",
          color: "#f6f5ef",
          gap: 24,
        }}
      >
        <strong
          style={{
            fontSize: 22,
            fontWeight: 500,
            opacity: interpolate(frame, [0, 12], [0, 1], {
              extrapolateRight: "clamp",
            }),
          }}
        >
          {title}
        </strong>
        <span style={{ marginLeft: "auto", fontSize: 14, color: "#aeb8ce" }}>
          {detail}
        </span>
      </div>
      <OffthreadVideo
        src={staticFile(`captures/${feature}-${lang}.webm`)}
        muted
        style={{ position: "absolute", top: 60, width: 1440, height: 900 }}
      />
    </AbsoluteFill>
  );
};
