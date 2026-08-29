import { describe, expect, it } from "vitest";

import {
  releaseMsiBuildConfig,
  releaseMsiProductVersion,
} from "./release-msi-build-config";

function compareMsiVersions(left: string, right: string): number {
  const components = (version: string) => version.split(".").map(Number);
  const leftParts = components(left);
  const rightParts = components(right);
  for (let index = 0; index < 3; index += 1) {
    const difference = leftParts[index] - rightParts[index];
    if (difference !== 0) return difference;
  }
  return 0;
}

describe("release MSI build config", () => {
  it("uses exactly three fields with monotonic beta and stable ordering", () => {
    const legacy = "0.0.8";
    const beta1 = releaseMsiProductVersion("0.0.9-beta.1");
    const beta2 = releaseMsiProductVersion("0.0.9-beta.2");
    const stable = releaseMsiProductVersion("0.0.9");
    const nextBeta = releaseMsiProductVersion("0.0.10-beta.1");

    expect(beta1).toBe("0.0.2305");
    expect(beta2).toBe("0.0.2306");
    expect(stable).toBe("0.0.2559");
    for (const version of [beta1, beta2, stable, nextBeta]) {
      expect(version.split(".")).toHaveLength(3);
    }
    expect(compareMsiVersions(legacy, beta1)).toBeLessThan(0);
    expect(compareMsiVersions(beta1, beta2)).toBeLessThan(0);
    expect(compareMsiVersions(beta2, stable)).toBeLessThan(0);
    expect(compareMsiVersions(stable, nextBeta)).toBeLessThan(0);
  });

  it("uses beta channels 1 through 254 and stable channel 255", () => {
    expect(releaseMsiProductVersion("1.2.3-beta.254")).toBe("1.2.1022");
    expect(releaseMsiProductVersion("1.2.3")).toBe("1.2.1023");
  });

  it("generates a numeric WiX override without changing stable updater behavior", () => {
    expect(releaseMsiBuildConfig("0.0.9-beta.2", true)).toEqual({
      bundle: {
        createUpdaterArtifacts: false,
        windows: { wix: { version: "0.0.2306" } },
      },
      plugins: { updater: { endpoints: [] } },
    });
    expect(releaseMsiBuildConfig("0.0.9")).toEqual({
      bundle: {
        windows: { wix: { version: "0.0.2559" } },
      },
    });
  });

  it.each(["0.0.0-beta.0", "0.0.0-beta.255", "256.0.0", "0.256.0", "0.0.256"])(
    "rejects unsupported MSI release version %s",
    (version) => {
      expect(() => releaseMsiProductVersion(version)).toThrow();
    },
  );
});
