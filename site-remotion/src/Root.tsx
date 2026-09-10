import "./index.css";
import { Composition } from "remotion";
import { RecordedWorkbench } from "./RecordedWorkbench";
import manifest from "./capture-manifest.json";

export const RemotionRoot: React.FC = () => (
  <>
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
