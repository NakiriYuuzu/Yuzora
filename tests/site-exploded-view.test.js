/* global DOMParser */

import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import process from "node:process";

import { describe, expect, it, vi } from "vitest";

import {
  PHASE_STOPS,
  activateExclusiveMedia,
  mediaForPhase,
  resolvePhase,
  sampleTrack,
} from "../site/exploded-view-model.js";

const root = process.cwd();
const htmlSource = readFileSync(resolve(root, "site/index.html"), "utf8");
const cssSource = readFileSync(resolve(root, "site/styles.css"), "utf8");
const jsSource = readFileSync(resolve(root, "site/exploded-view.js"), "utf8");
const page = new DOMParser().parseFromString(htmlSource, "text/html");

function localReferenceExists(reference, baseFile = "site/index.html") {
  if (
    !reference ||
    reference.startsWith("#") ||
    /^(?:https?:|data:|mailto:)/.test(reference)
  )
    return true;
  return existsSync(resolve(root, dirname(baseFile), reference));
}

describe("GitHub Pages Exploded View", () => {
  it("publishes the static CSS, timeline module and download module", () => {
    expect(page.querySelector('link[href="./styles.css"]')).not.toBeNull();
    expect(
      page.querySelector('script[src="./exploded-view.js"]'),
    ).not.toBeNull();
    expect(existsSync(resolve(root, "site/exploded-view-model.js"))).toBe(true);
    expect(jsSource).toContain('from "./exploded-view-model.js"');
    expect(
      [...page.querySelectorAll('script[type="module"]')].some((script) =>
        script.textContent?.includes('from "./downloads.js"'),
      ),
    ).toBe(true);
  });

  it("contains seven live phases, seven keyboard jumps and seven fallback chapters", () => {
    expect(page.querySelectorAll("[data-phase-copy]")).toHaveLength(7);
    expect(page.querySelectorAll("button[data-phase-jump]")).toHaveLength(7);
    expect(
      page.querySelectorAll("[data-story-fallback] .chapter"),
    ).toHaveLength(7);
    expect(PHASE_STOPS).toEqual([0, 0.14, 0.3, 0.46, 0.62, 0.78, 0.94]);
    expect(page.querySelector('[data-phase-jump="6"] b')?.textContent).toBe(
      "REASSEMBLE",
    );
  });

  it("uses one requestAnimationFrame-throttled scroll driver", () => {
    expect(jsSource.match(/addEventListener\("scroll"/g)).toHaveLength(1);
    expect(jsSource).toContain("window.requestAnimationFrame(updateStory)");
    expect(
      cssSource.match(/\.story-pin\s*\{\s*position:\s*sticky;/g),
    ).toHaveLength(1);
  });

  it("resolves phase boundaries and interpolates tracks", () => {
    expect(resolvePhase(0)).toBe(0);
    expect(resolvePhase(0.07)).toBe(0);
    expect(resolvePhase(0.071)).toBe(1);
    expect(resolvePhase(0.22)).toBe(1);
    expect(resolvePhase(0.221)).toBe(2);
    expect(resolvePhase(1)).toBe(6);

    const track = PHASE_STOPS.map((_, index) => [index * 10]);
    expect(sampleTrack(track, PHASE_STOPS[2])).toEqual([20]);
    expect(
      sampleTrack(track, (PHASE_STOPS[0] + PHASE_STOPS[1]) / 2)[0],
    ).toBeCloseTo(5);
  });

  it("maps runtime media only to phases 2 through 5", () => {
    expect([0, 1, 2, 3, 4, 5, 6].map(mediaForPhase)).toEqual([
      null,
      null,
      "ade-herdr",
      "ade-herdr",
      "remote-db",
      "terminal-git",
      null,
    ]);
  });

  it("pauses inactive media and contains rejected active playback", async () => {
    const makeVideo = (media, play = vi.fn().mockResolvedValue(undefined)) => ({
      dataset: { media },
      hidden: false,
      pause: vi.fn(),
      play,
    });
    const rejectedPlay = vi
      .fn()
      .mockRejectedValue(new Error("autoplay blocked"));
    const videos = [
      makeVideo("ade-herdr"),
      makeVideo("remote-db", rejectedPlay),
      makeVideo("terminal-git"),
    ];
    const onPlayError = vi.fn();

    await expect(
      activateExclusiveMedia(videos, "remote-db", { onPlayError }),
    ).resolves.toBe(videos[1]);
    expect(videos[0].hidden).toBe(true);
    expect(videos[0].pause).toHaveBeenCalledOnce();
    expect(videos[1].hidden).toBe(false);
    expect(rejectedPlay).toHaveBeenCalledOnce();
    expect(onPlayError).toHaveBeenCalledOnce();
    expect(videos[2].hidden).toBe(true);
    expect(videos[2].pause).toHaveBeenCalledOnce();
  });

  it("ships a distinct four-surface final hierarchy in live and fallback states", () => {
    const expected = ["runtime", "inspector", "remote-db", "terminal-git"];
    expect(
      [...page.querySelectorAll(".final-collage [data-visual]")].map(
        (image) => image.dataset.visual,
      ),
    ).toEqual(expected);
    expect(
      [...page.querySelectorAll(".fallback-final-collage [data-visual]")].map(
        (image) => image.dataset.visual,
      ),
    ).toEqual(expected);
  });

  it("keeps no-JS, mobile and reduced-motion fallbacks", () => {
    expect(page.documentElement.classList.contains("no-js")).toBe(true);
    expect(page.querySelector("[data-story-fallback]")).not.toBeNull();
    expect(cssSource).toContain("@media (max-width: 1023px)");
    expect(cssSource).toContain("@media (prefers-reduced-motion: reduce)");
    expect(cssSource).toMatch(
      /\.exploded-story\s*\{\s*height:\s*auto !important;/,
    );
  });

  it("keeps inactive media out of layout and preserves product aspect ratios", () => {
    expect(page.querySelectorAll("[data-live-video][hidden]")).toHaveLength(3);
    expect(cssSource).toMatch(
      /\[hidden\]\s*\{\s*display:\s*none !important;\s*\}/,
    );
    expect(cssSource).toMatch(
      /\.product-image,\s*\.chapter img,\s*\.chapter video\s*\{[^}]*height:\s*auto;/s,
    );
  });

  it("uses a compact desktop contract before the full exploded layout has room", () => {
    expect(cssSource).toMatch(
      /@media \(min-width: 1024px\) and \(max-width: 1199px\)\s*\{[\s\S]*?\.connector,\s*\.measure-side\s*\{\s*display:\s*none;/,
    );
    expect(cssSource).toMatch(
      /@media \(min-width: 1024px\) and \(max-width: 1199px\)\s*\{[\s\S]*?\.phase-index button\s*\{\s*min-height:\s*50px;/,
    );
  });

  it("localizes landmark labels and keeps branding on the leaf-green accent", () => {
    const labelledSelectors = [
      "nav[data-i18n-aria-label]",
      ".brand[data-i18n-aria-label]",
      ".lang-switch[data-i18n-aria-label]",
      "[data-story][data-i18n-aria-label]",
      ".phase-index[data-i18n-aria-label]",
    ];
    for (const selector of labelledSelectors) {
      expect(page.querySelector(selector), selector).not.toBeNull();
    }
    expect(
      page.querySelector('link[rel="icon"]')?.getAttribute("href"),
    ).toContain("%2386b81f");
    expect(cssSource).toMatch(
      /\.brand-logo\s*\{[^}]*background:\s*var\(--accent\)/s,
    );
  });

  it("provides both localized dictionary entries for every markup key", () => {
    const attributes = [
      "data-i18n",
      "data-i18n-html",
      "data-i18n-alt",
      "data-i18n-aria-label",
    ];
    const keys = new Set(
      attributes.flatMap((attribute) =>
        [...page.querySelectorAll(`[${attribute}]`)].map((element) =>
          element.getAttribute(attribute),
        ),
      ),
    );

    for (const key of keys) {
      const escaped = key.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
      expect(
        jsSource.match(new RegExp(`(?:"${escaped}"|${escaped}):`, "g"))?.length,
        key,
      ).toBe(2);
    }
  });

  it("references only local assets that are present in the Pages artifact", () => {
    const references = [
      ...page.querySelectorAll("[href], [src], [poster]"),
    ].flatMap((element) =>
      ["href", "src", "poster"]
        .map((attribute) => element.getAttribute(attribute))
        .filter(Boolean),
    );
    for (const reference of references)
      expect(localReferenceExists(reference), reference).toBe(true);
  });

  it("ships every dynamically selected zh/en video and still", () => {
    const videoStems = ["ade-herdr", "remote-db", "terminal-git"];
    const stillStems = [
      "ade-herdr-runtime",
      "ade-herdr-inspector",
      "remote-db",
      "terminal-git",
    ];
    for (const lang of ["zh", "en"]) {
      for (const stem of videoStems) {
        expect(
          existsSync(resolve(root, `site/assets/${stem}-${lang}.mp4`)),
        ).toBe(true);
      }
      for (const stem of stillStems) {
        expect(
          existsSync(resolve(root, `site/assets/${stem}-${lang}.png`)),
        ).toBe(true);
      }
    }
  });
});
