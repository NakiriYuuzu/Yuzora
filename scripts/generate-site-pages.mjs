import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath, URL } from "node:url";
import process from "node:process";
import { JSDOM } from "jsdom";
import { I18N } from "../site/i18n.js";

const siteDirectory = new URL("../site/", import.meta.url);
const siteUrl = "https://github.yuuzu.net/Yuzora/";
const repositoryUrl = "https://github.com/NakiriYuuzu/Yuzora";
const locales = { "zh-Hant": siteUrl, en: `${siteUrl}en/` };
const seoRegion = /<!-- seo:start -->[\s\S]*?<!-- seo:end -->/;
const escapeHtml = (value) => value.replaceAll("&", "&amp;").replaceAll('"', "&quot;").replaceAll("<", "&lt;").replaceAll(">", "&gt;");

function seoHead(lang) {
  const dict = I18N[lang];
  const canonical = locales[lang];
  const image = `${siteUrl}assets/ade-herdr-runtime-${lang === "en" ? "en" : "zh"}.png`;
  const schema = {
    "@context": "https://schema.org",
    "@type": "SoftwareApplication",
    "@id": `${siteUrl}#application`,
    name: "Yuzora",
    url: canonical,
    description: dict["meta.description"],
    applicationCategory: "DeveloperApplication",
    operatingSystem: ["macOS (Apple Silicon)", "Windows (x64)"],
    inLanguage: ["zh-Hant", "en"],
    image,
    screenshot: image,
    downloadUrl: `${canonical}#download`,
    sameAs: repositoryUrl,
    author: { "@type": "Person", name: "NakiriYuuzu", url: "https://github.com/NakiriYuuzu" },
  };
  return `<!-- seo:start -->
<title>${escapeHtml(dict["meta.title"])}</title>
<meta name="description" content="${escapeHtml(dict["meta.description"])}">
<meta name="robots" content="index, follow, max-image-preview:large">
<link rel="canonical" href="${canonical}">
${Object.entries(locales).map(([locale, href]) => `<link rel="alternate" hreflang="${locale}" href="${href}">`).join("\n")}
<link rel="alternate" hreflang="x-default" href="${siteUrl}">
<meta property="og:type" content="website">
<meta property="og:site_name" content="Yuzora">
<meta property="og:url" content="${canonical}">
<meta property="og:locale" content="${lang === "en" ? "en_US" : "zh_TW"}">
<meta property="og:locale:alternate" content="${lang === "en" ? "zh_TW" : "en_US"}">
<meta property="og:title" content="${escapeHtml(dict["meta.title"])}">
<meta property="og:description" content="${escapeHtml(dict["meta.ogDescription"])}">
<meta property="og:image" content="${image}">
<meta property="og:image:width" content="1440">
<meta property="og:image:height" content="960">
<meta property="og:image:alt" content="${escapeHtml(dict["hero.alt"])}">
<meta name="twitter:card" content="summary_large_image">
<meta name="twitter:title" content="${escapeHtml(dict["meta.title"])}">
<meta name="twitter:description" content="${escapeHtml(dict["meta.ogDescription"])}">
<meta name="twitter:image" content="${image}">
<meta name="twitter:image:alt" content="${escapeHtml(dict["hero.alt"])}">
<script type="application/ld+json">${JSON.stringify(schema).replaceAll("<", "\\u003c")}</script>
<!-- seo:end -->`;
}

const template = readFileSync(new URL("index.html", siteDirectory), "utf8");
if (!seoRegion.test(template)) throw new Error("site/index.html is missing its SEO markers");
// Keep the hand-authored source readable; only the SEO region is generated here.
const chinesePage = template.replace(seoRegion, () => seoHead("zh-Hant"));
writeFileSync(new URL("index.html", siteDirectory), chinesePage);

const dom = new JSDOM(template.replace(seoRegion, () => seoHead("en")));
const document = dom.window.document;
document.documentElement.lang = "en";
for (const [attribute, target] of Object.entries({
  "data-i18n": "textContent",
  "data-i18n-html": "innerHTML",
  "data-i18n-alt": "alt",
  "data-i18n-placeholder": "placeholder",
  "data-i18n-aria-label": "aria-label",
})) {
  for (const element of document.querySelectorAll(`[${attribute}]`)) {
    const key = element.getAttribute(attribute);
    const value = I18N.en[key];
    if (value === undefined) throw new Error(`Missing English translation: ${key}`);
    const translated = value.replace("{{count}}", "0");
    if (target === "textContent" || target === "innerHTML") element[target] = translated;
    else element.setAttribute(target, translated);
  }
}
for (const image of document.querySelectorAll("img[data-imgstem]")) {
  image.setAttribute("src", `assets/${image.dataset.imgstem}-en.png`);
}
for (const video of document.querySelectorAll("video[data-vstem]")) {
  video.setAttribute("poster", `assets/${video.dataset.posterStem}-en.png`);
  video.querySelector("source").setAttribute("src", `assets/${video.dataset.vstem}-en.mp4`);
}
// No <base>: fragment navigation and inline SVG references must stay on /en/.
for (const element of document.querySelectorAll("[href], [src], [poster]")) {
  for (const attribute of ["href", "src", "poster"]) {
    const value = element.getAttribute(attribute);
    if (value && !/^(?:[a-z][a-z\d+.-]*:|\/|#)/i.test(value)) {
      element.setAttribute(attribute, `../${value.replace(/^\.\//, "")}`);
    }
  }
}
for (const link of document.querySelectorAll("[data-demo-link]")) {
  link.setAttribute("href", "../demo/?lang=en");
}
const languageLink = document.getElementById("lang-toggle");
languageLink.setAttribute("href", "../");
languageLink.setAttribute("lang", "zh-Hant");
languageLink.setAttribute("hreflang", "zh-Hant");
languageLink.setAttribute("aria-label", "閱讀繁體中文版");
languageLink.textContent = "中文";
mkdirSync(new URL("en/", siteDirectory), { recursive: true });
writeFileSync(new URL("en/index.html", siteDirectory), `${dom.serialize()}\n`);
dom.window.close();

const alternates = [...Object.entries(locales), ["x-default", siteUrl]]
  .map(([lang, href]) => `    <xhtml:link rel="alternate" hreflang="${lang}" href="${href}"/>`).join("\n");
writeFileSync(new URL("sitemap.xml", siteDirectory), `<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:xhtml="http://www.w3.org/1999/xhtml">
${Object.values(locales).map((url) => `  <url>\n    <loc>${url}</loc>\n${alternates}\n  </url>`).join("\n")}
</urlset>
`);
// The English page and sitemap are deployment artifacts, like site/demo/.
process.stdout.write(`Generated localized Pages and sitemap in ${fileURLToPath(siteDirectory)}\n`);
