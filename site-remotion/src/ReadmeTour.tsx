import { Series } from "remotion";
import { RecordedWorkbench } from "./RecordedWorkbench";

const features = ["ade-herdr", "terminal-git", "remote-db"] as const;
export const README_TOUR_FRAMES = 210 * features.length;

/** Reuses the same current AppShell recordings as the Pages feature videos. */
export const ReadmeTour: React.FC<{ lang: "zh" | "en" }> = ({ lang }) => (
  <Series>
    {features.map((feature) => (
      <Series.Sequence key={feature} durationInFrames={210}>
        <RecordedWorkbench feature={feature} lang={lang} />
      </Series.Sequence>
    ))}
  </Series>
);
