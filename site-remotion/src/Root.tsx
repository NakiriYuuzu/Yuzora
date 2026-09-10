import "./index.css";
import { Composition } from "remotion";
import { RecordedWorkbench } from "./RecordedWorkbench";
import manifest from "./capture-manifest.json";
import { ReadmeTour, README_TOUR_FRAMES } from "./ReadmeTour";

export const RemotionRoot: React.FC = () => (
  <>
    {(["zh", "en"] as const).map((lang) => (
      <Composition
        key={`readme-tour-${lang}`}
        id={`readme-tour-${lang}`}
        component={ReadmeTour}
        defaultProps={{ lang }}
        durationInFrames={README_TOUR_FRAMES}
        fps={30}
        width={1440}
        height={960}
      />
    ))}
    {manifest.map(({ feature, lang, frames }) => (
      <Composition
        key={`${feature}-${lang}`}
        id={`${feature}-${lang}`}
        component={RecordedWorkbench}
        defaultProps={{
          feature: feature as "ade-herdr" | "terminal-git" | "remote-db",
          lang: lang as "zh" | "en",
        }}
        durationInFrames={frames}
        fps={30}
        width={1440}
        height={960}
      />
    ))}
  </>
);
