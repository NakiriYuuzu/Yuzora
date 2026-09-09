// Local visual identity. Never used as Agent persona, prompt or runtime state.
export const CHARACTER_SHELLS = ["round", "box", "cloud"] as const;
export const CHARACTER_FACES = ["curious", "smile", "sleepy"] as const;
export const CHARACTER_DETAILS = ["none", "freckles", "patch"] as const;
export interface SpaceCharacterConfig {
  shell: (typeof CHARACTER_SHELLS)[number];
  face: (typeof CHARACTER_FACES)[number];
  detail: (typeof CHARACTER_DETAILS)[number];
  motion?: boolean;
}
export const DEFAULT_SPACE_CHARACTER: SpaceCharacterConfig = {
  shell: "round",
  face: "curious",
  detail: "none",
};

export function sanitizeSpaceCharacter(
  value: unknown,
): SpaceCharacterConfig | undefined {
  if (!value || typeof value !== "object") return undefined;
  const candidate = value as Record<string, unknown>;
  const shell = candidate.shell === "bean" ? "cloud" : candidate.shell;
  if (
    !CHARACTER_SHELLS.some((item) => item === shell) ||
    !CHARACTER_FACES.some((item) => item === candidate.face) ||
    !CHARACTER_DETAILS.some((item) => item === candidate.detail)
  )
    return undefined;
  return {
    shell,
    face: candidate.face,
    detail: candidate.detail,
    motion: candidate.motion !== false,
  } as SpaceCharacterConfig;
}
