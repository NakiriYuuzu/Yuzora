import "./index.css";
import { Composition } from "remotion";
import { AgentZone } from "./AgentZone";
import { HERO_DURATION, Hero } from "./Hero";
import { RemoteDb } from "./RemoteDb";
import { TerminalGit } from "./TerminalGit";

const FPS = 30;
const WIDTH = 1280;
const HEIGHT = 800;

const LANGS = ["zh", "en"] as const;

export const RemotionRoot: React.FC = () => {
  return (
    <>
      {LANGS.map((lang) => (
        <Composition
          key={`hero-${lang}`}
          id={`hero-${lang}`}
          component={Hero}
          defaultProps={{ lang }}
          durationInFrames={HERO_DURATION}
          fps={FPS}
          width={WIDTH}
          height={HEIGHT}
        />
      ))}
      {LANGS.map((lang) => (
        <Composition
          key={`agentzone-${lang}`}
          id={`agentzone-${lang}`}
          component={AgentZone}
          defaultProps={{ lang }}
          durationInFrames={290}
          fps={FPS}
          width={WIDTH}
          height={HEIGHT}
        />
      ))}
      {LANGS.map((lang) => (
        <Composition
          key={`remote-db-${lang}`}
          id={`remote-db-${lang}`}
          component={RemoteDb}
          defaultProps={{ lang }}
          durationInFrames={230}
          fps={FPS}
          width={WIDTH}
          height={HEIGHT}
        />
      ))}
      {LANGS.map((lang) => (
        <Composition
          key={`terminal-git-${lang}`}
          id={`terminal-git-${lang}`}
          component={TerminalGit}
          defaultProps={{ lang }}
          durationInFrames={240}
          fps={FPS}
          width={WIDTH}
          height={HEIGHT}
        />
      ))}
    </>
  );
};
