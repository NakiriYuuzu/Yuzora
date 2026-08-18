import { assertReleaseVersion } from "./release-version";

const MAX_MSI_MAJOR_OR_MINOR = 255;
const MAX_MSI_BUILD = 65_535;
const PATCH_BUILD_STRIDE = 256;
const STABLE_CHANNEL = 255;
const MAX_BETA_SEQUENCE = STABLE_CHANNEL - 1;

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

function numericComponent(
  value: string,
  name: string,
  maximum: number,
): number {
  const parsed = Number(value);
  assert(
    Number.isSafeInteger(parsed) && parsed >= 0 && parsed <= maximum,
    `${name} must be an integer between 0 and ${maximum} for an MSI ProductVersion`,
  );
  return parsed;
}

/**
 * Maps a supported product release to Windows Installer's three-component
 * ProductVersion. The build component reserves 1..254 for beta.N and 255 for
 * the final stable release of each semantic patch, so MSI ordering remains
 * monotonic without relying on the ignored fourth ProductVersion field.
 */
export function releaseMsiProductVersion(version: string): string {
  const channel = assertReleaseVersion(version);
  const match = /^(\d+)\.(\d+)\.(\d+)(?:-beta\.(\d+))?$/.exec(version);
  assert(match, `could not parse release version ${version}`);

  const major = numericComponent(match[1], "major", MAX_MSI_MAJOR_OR_MINOR);
  const minor = numericComponent(match[2], "minor", MAX_MSI_MAJOR_OR_MINOR);
  const patch = numericComponent(match[3], "patch", MAX_MSI_MAJOR_OR_MINOR);
  const beta =
    channel === "beta"
      ? numericComponent(match[4] ?? "", "beta sequence", MAX_BETA_SEQUENCE)
      : STABLE_CHANNEL;
  assert(
    channel !== "beta" || beta > 0,
    "beta sequence must be greater than zero",
  );

  const build = patch * PATCH_BUILD_STRIDE + beta;
  assert(build <= MAX_MSI_BUILD, "MSI build component exceeds 65535");
  return `${major}.${minor}.${build}`;
}

export function releaseMsiBuildConfig(
  version: string,
  disableUpdaterArtifacts = false,
) {
  return {
    bundle: {
      ...(disableUpdaterArtifacts ? { createUpdaterArtifacts: false } : {}),
      windows: {
        wix: {
          version: releaseMsiProductVersion(version),
        },
      },
    },
    ...(disableUpdaterArtifacts
      ? { plugins: { updater: { endpoints: [] as string[] } } }
      : {}),
  };
}

if (import.meta.main) {
  try {
    const [version, option] = process.argv.slice(2);
    if (!version)
      throw new Error("release product version argument is required");
    if (option !== undefined && option !== "--no-updater") {
      throw new Error(`unsupported build config option ${option}`);
    }
    console.log(
      JSON.stringify(releaseMsiBuildConfig(version, option === "--no-updater")),
    );
  } catch (error) {
    console.error(
      `::error::${error instanceof Error ? error.message : String(error)}`,
    );
    process.exit(1);
  }
}
