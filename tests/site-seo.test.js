/* global DOMParser */
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { URL } from "node:url";
import process from "node:process";
import { describe, expect, it } from "vitest";
import { I18N } from "../site/i18n.js";

const site = resolve(process.cwd(), "site");
const base = "https://github.yuuzu.net/Yuzora/";
const locales = { "zh-Hant": "", en: "en/" };
const parse = (html) => new DOMParser().parseFromString(html, "text/html");

describe("crawlable bilingual product pages", () => {
  for (const [lang, path] of Object.entries(locales)) {
    const page = parse(readFileSync(resolve(site, path, "index.html"), "utf8"));
    const canonical = `${base}${path}`;

    it(`${lang} serves translated content and accessibility labels before JavaScript`, () => {
      expect(page.documentElement.lang).toBe(lang);
      expect(page.title).toBe(I18N[lang]["meta.title"]);
      expect(page.querySelectorAll("h1")).toHaveLength(1);
      expect(page.querySelector("h1").textContent).toContain("Yuzora");
      for (const [attr, content] of Object.entries({
        "data-i18n": "textContent", "data-i18n-html": "innerHTML",
        "data-i18n-alt": "alt", "data-i18n-placeholder": "placeholder", "data-i18n-aria-label": "aria-label",
      })) {
        for (const element of page.querySelectorAll(`[${attr}]`)) {
          const expected = I18N[lang][element.getAttribute(attr)].replace("{{count}}", "0");
          const actual = ["textContent", "innerHTML"].includes(content) ? element[content] : element.getAttribute(content);
          expect(actual, `${lang}: ${element.getAttribute(attr)}`).toBe(expected);
        }
      }
      expect(page.querySelector('meta[name="robots"]').content).toContain("index, follow");
      expect(page.querySelector("base")).toBeNull();
    });

    it(`${lang} has self canonical, reciprocal language links and absolute social previews`, () => {
      expect(page.querySelectorAll('link[rel="canonical"]')).toHaveLength(1);
      expect(page.querySelector('link[rel="canonical"]').href).toBe(canonical);
      for (const [locale, suffix] of Object.entries({ ...locales, "x-default": "" })) {
        expect(page.querySelector(`link[hreflang="${locale}"]`).href).toBe(`${base}${suffix}`);
      }
      expect(page.querySelector('meta[property="og:url"]').content).toBe(canonical);
      expect(page.querySelector('meta[name="description"]').content).toBe(I18N[lang]["meta.description"]);
      for (const selector of ['meta[property="og:image"]', 'meta[name="twitter:image"]']) {
        const url = page.querySelector(selector).content;
        expect(url).toBe(`${base}assets/ade-herdr-runtime-${lang === "en" ? "en" : "zh"}.png`);
        expect(existsSync(resolve(site, url.slice(base.length)))).toBe(true);
      }
      expect(page.querySelector('meta[name="twitter:card"]').content).toBe("summary_large_image");
      const schema = JSON.parse(page.querySelector('script[type="application/ld+json"]').textContent);
      expect(schema).toMatchObject({ "@type": "SoftwareApplication", name: "Yuzora", url: canonical });
      expect(schema.operatingSystem).toEqual(["macOS (Apple Silicon)", "Windows (x64)"]);
      expect(schema.sameAs).toBe("https://github.com/NakiriYuuzu/Yuzora");
      expect(schema.aggregateRating).toBeUndefined();
    });

    it(`${lang} keeps resources, fragments and language navigation valid under the repository subpath`, () => {
      const otherPath = lang === "en" ? "" : "en/";
      expect(new URL(page.getElementById("lang-toggle").getAttribute("href"), canonical).href).toBe(`${base}${otherPath}`);
      for (const element of page.querySelectorAll("[href], [src], [poster]")) {
        for (const attr of ["href", "src", "poster"]) {
          const reference = element.getAttribute(attr);
          if (!reference) continue;
          const url = new URL(reference, canonical);
          if (reference.startsWith("#")) {
            expect(url.href.startsWith(canonical)).toBe(true);
            expect(page.getElementById(url.hash.slice(1)), reference).not.toBeNull();
          } else if (url.href.startsWith(base)) {
            const relativePath = url.pathname.slice(new URL(base).pathname.length);
            expect(existsSync(resolve(site, relativePath)), reference).toBe(true);
          }
        }
      }
      for (const link of page.querySelectorAll("[data-demo-link]")) {
        const demo = new URL(link.getAttribute("href"), canonical);
        expect(demo.pathname).toBe("/Yuzora/demo/");
        if (lang === "en") expect(demo.searchParams.get("lang")).toBe("en");
      }
      for (const image of page.querySelectorAll("img[data-imgstem]")) {
        expect(image.getAttribute("src")).toContain(`-${lang === "en" ? "en" : "zh"}.png`);
      }
    });
  }

  it("lists only canonical product pages in a valid reciprocal sitemap", () => {
    const sitemap = new DOMParser().parseFromString(readFileSync(resolve(site, "sitemap.xml"), "utf8"), "application/xml");
    expect(sitemap.querySelector("parsererror")).toBeNull();
    const entries = [...sitemap.getElementsByTagName("url")];
    expect(entries.map(entry => entry.querySelector("loc").textContent)).toEqual([base, `${base}en/`]);
    for (const entry of entries) {
      expect([...entry.getElementsByTagNameNS("http://www.w3.org/1999/xhtml", "link")].map(link => [link.getAttribute("hreflang"), link.getAttribute("href")]))
        .toEqual([["zh-Hant", base], ["en", `${base}en/`], ["x-default", base]]);
    }
    expect(sitemap.querySelector("lastmod")).toBeNull();
  });

  it("keeps the sample-data demo out of search results while allowing its links to be followed", () => {
    const demo = parse(readFileSync(resolve(site, "demo/index.html"), "utf8"));
    expect(demo.querySelector('meta[name="robots"]').content).toBe("noindex, follow");
  });
});
