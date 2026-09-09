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
  return (
    <span
      className="space-character-art"
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
                  <path d="M37 52q3 3 6 0" strokeWidth="2.2" />
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
          </g>
        </svg>
      </span>
    </span>
  );
}
