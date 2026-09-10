import { useState, type CSSProperties } from "react";
import type { SpaceCharacterConfig } from "./space-character";

/** Original modular avatar art inspired by soft shapes and minimal faces.
 * Expression is a visual preference, never an agent state projection.
 */
export function SpaceCharacter({
  character,
  portrait = false,
}: {
  character: SpaceCharacterConfig;
  portrait?: boolean;
}) {
  const { shell, face, detail } = character;
  // Keep each companion's independent rhythm stable across parent re-renders.
  const [motionStyle] = useState(() => ({
    "--character-blink-duration": `${4 + Math.random() * 3}s`,
    "--character-blink-phase": `${-Math.random() * 7}s`,
    "--character-look-duration": `${9 + Math.random() * 6}s`,
    "--character-look-phase": `${-Math.random() * 15}s`,
    "--character-body-duration": `${5 + Math.random() * 4}s`,
    "--character-body-phase": `${-Math.random() * 9}s`,
    "--character-body-animation": ["character-bob", "character-sway", "character-nod"][Math.floor(Math.random() * 3)],
  }) as CSSProperties);
  return (
    <span
      className="space-character-art"
      style={motionStyle}
      data-shell={shell}
      data-face={face}
      data-detail={detail}
      data-portrait={portrait}
      data-motion={character.motion !== false}
      aria-hidden="true"
    >
      <span className="space-character-body">
        <span className="space-character-pattern" />
        <svg
          className="space-character-face"
          viewBox="0 0 80 80"
          fill="none"
          focusable="false"
        >
          <g className="character-gaze">
            <g className="character-blink">
              {face === "curious" && (
                <g fill="currentColor">
                  <ellipse
                    cx="29"
                    cy="43"
                    rx="3.2"
                    ry="6.5"
                    transform="rotate(-8 29 43)"
                  />
                  <ellipse
                    cx="51"
                    cy="41"
                    rx="3.2"
                    ry="6.5"
                    transform="rotate(-8 51 41)"
                  />
                </g>
              )}
              {face === "smile" && (
                <g
                  stroke="currentColor"
                  strokeWidth="3.5"
                  strokeLinecap="round"
                >
                  <path d="M24 44q5-9 10 0M46 44q5-9 10 0" />
                </g>
              )}
              {face === "sleepy" && (
                <g
                  stroke="currentColor"
                  strokeWidth="3.5"
                  strokeLinecap="round"
                >
                  <path d="m25 42 9 1m12 0 9-1" />
                </g>
              )}
            </g>
            <g className="character-mouth" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round">
              {face === "curious" && <path d="M37 53q3 4 6 0" />}
              {face === "smile" && <path d="M35 52q5 7 10 0" />}
              {face === "sleepy" && <ellipse cx="40" cy="54" rx="2.5" ry="1.5" />}
            </g>
          </g>
        </svg>
      </span>
    </span>
  );
}
