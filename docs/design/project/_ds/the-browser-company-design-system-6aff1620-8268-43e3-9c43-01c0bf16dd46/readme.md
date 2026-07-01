# The Browser Company — Design System

A design system in the spirit of **The Browser Company of New York** — makers of **Arc** and **Dia**. Warm, optimistic, editorial, and human. Software that feels less like a tool and more like a place. This system captures the brand's signature gradient spectrum, cream-paper surfaces, editorial serif headlines, rounded friendly chrome, and springy, delightful motion.

> **Sources.** No codebase or Figma file was attached to this project. This system was reconstructed from established public brand characteristics of Arc and Dia (the rainbow "Arc gradient", warm off-white canvases, the rounded sidebar-first browser chrome, editorial serif voice). **If you have the real codebase, Figma, or brand fonts, re-attach them via the Import menu and I'll align everything precisely.**

---

## 1. Context

The Browser Company builds browsers that reimagine the relationship between people and the internet.

- **Arc** — a sidebar-first, space-organized browser. Tabs live on the left, auto-archive, and are grouped into colorful "Spaces", each with its own gradient theme. Known for the command bar (⌘T / ⌘L), Easels, Boosts, and a deeply playful, customizable feel.
- **Dia** — the newer AI-native browser. Calmer, lighter, chat-forward: an AI assistant lives alongside browsing, summarizing tabs and answering questions in context. Cleaner and more restrained than Arc, but sharing the same warm DNA.

The two products are siblings: Arc is expressive and maximalist; Dia is quiet and focused. This system serves both — a warm neutral foundation with an expressive gradient layer that each product dials up or down.

---

## 2. Content Fundamentals

**Voice:** human, warm, a little literary. The Browser Company writes like a thoughtful friend, not a corporation. Sentences are confident and plainspoken; copy often addresses **"you"** directly and speaks as **"we"** with genuine warmth.

- **Tone:** optimistic, curious, calm. Never hype-y or jargon-filled. Comfortable with a poetic turn of phrase ("the internet is a place", "a browser that feels like you").
- **Casing:** **Sentence case** everywhere — buttons, menus, titles, headings. Avoid Title Case and ALL-CAPS (except tiny micro-labels with tracked letterspacing).
- **Person:** "you" for the reader; "we" for the company. First-person product voice is fine ("Let's set up your first Space").
- **Punctuation:** real em-dashes, the occasional ellipsis for warmth. Periods optional on short UI labels.
- **Emoji:** used **sparingly and intentionally** — Spaces get a single emoji/icon as identity; marketing rarely uses emoji in body copy. Never decorative emoji-spam.
- **Length:** short. Headlines are a single idea. Body copy breathes. Empty states and onboarding are encouraging, never clinical.

**Examples**
- Button: `New Space` · `Continue` · `Maybe later` · `Add to Space`
- Empty state: `Nothing here yet. Open a tab to get started.`
- Onboarding: `Welcome back. Pick up where you left off?`
- Dia assistant: `I read this page for you — here's the gist.`

Avoid: "Click here", "Submit", "Error!", Title-Cased Buttons, exclamation-heavy hype.

---

## 3. Visual Foundations

**Color.** A warm **cream paper** canvas (`--paper-1` `#fbfaf6`) instead of stark white — the brand never feels clinical. Ink is warm near-black, never pure `#000`. The hero motif is the **Arc gradient**: a left-to-right sweep through coral → magenta → violet → blue (`--grad-arc`). Each Space pulls a duotone slice of that spectrum. Soft pastel tints (`--*-soft`) back badges and highlights. Dark chrome ("night") is a warm charcoal, never blue-black.

**Type.** Editorial **serif** (Newsreader substitute) for display and headlines — this is what gives the brand its literary, human warmth. Warm humanist **sans** (Hanken Grotesk substitute) for all UI and body. **Mono** (JetBrains Mono) for URLs, shortcuts, and technical chrome. Display sizes are large with tight negative tracking; body is generous (1.55 line height).

**Spacing.** 8px rhythm with a 4px half-step. Generous padding — the UI is roomy and unhurried, never dense.

**Backgrounds.** Mostly flat warm cream or soft paper wells. Gradients appear as *intentional accents* — Space themes, hero panels, onboarding — applied via `--grad-arc` / `--grad-mesh`, never as a default page wash. Subtle radial mesh (`--grad-mesh`) for hero/empty-state atmosphere. No photographic textures or grain by default.

**Animation.** Springy and alive. Default settle uses `--ease-out`; playful elements (toggles, new-tab pop, sidebar reveals) use `--ease-spring` with a gentle overshoot. Durations 120–380ms. Things fade *and* move; nothing snaps. Reduced-motion is respected.

**Hover / press.** Hover lifts subtly (lighter fill / faint shadow bump). Press *shrinks* slightly (`scale(0.97)`) and darkens — a tactile, physical feel. Ghost controls darken their wash on hover rather than changing color.

**Borders & dividers.** Hairlines are warm translucent ink (`--line-1`), never gray. Many surfaces use shadow + radius alone, no border. Inputs get a 1px line that thickens to accent on focus, plus a soft focus ring (`--ring`).

**Shadows.** Soft, diffuse, warm-tinted (rgba of the ink, never pure black). Layered two-stop shadows for elevation (`--shadow-md`/`-lg`). Accent/gradient elements get a colored glow (`--glow-accent`).

**Translucency & blur.** Floating chrome (command bar, popovers, the Dia assistant panel) uses **frosted glass** — `--frost-light` background + `--blur-frost` backdrop-filter. Used only for things that float above content.

**Corner radii.** Rounded and friendly. Controls `--r-md` (14px), cards `--r-lg` (20px), hero panels `--r-xl` (28px), and full **pills** (`--r-pill`) for tags, toggles, and primary CTAs.

**Cards.** White or paper surface, `--r-lg` rounding, `--shadow-sm`/`-md`, usually **no border** (shadow defines the edge). Generous internal padding (`--sp-5`/`--sp-6`).

---

## 4. Iconography

The Browser Company uses a **clean, rounded line-icon style** with a medium stroke weight and rounded caps/joins — friendly, not technical. Spaces and profiles are identified by a single **emoji or symbol** chosen by the user.

- **Substitution (flagged):** the real Arc/Dia icon set isn't available here, so this system links **[Lucide](https://lucide.dev)** from CDN — its rounded 2px-stroke geometry is the closest open match to the Arc aesthetic. Swap for the real set if you have it.
- **Usage:** inline SVG via Lucide; default stroke `1.75–2`, rounded line caps. Size icons at 16/18/20/24. Tint with `currentColor` so they inherit text color.
- **Emoji:** allowed as **Space/profile identity glyphs** only (one per Space). Not used decoratively in body copy.
- **Unicode:** keyboard shortcut glyphs (⌘ ⇧ ⌥ ⏎) appear in mono in menus and the command bar.

---

## 5. Index / Manifest

**Root**
- `styles.css` — global entry point (import-only). Link this.
- `readme.md` — this file.
- `SKILL.md` — Agent Skill manifest.

**Tokens** (`tokens/`)
- `colors.css` · `typography.css` · `spacing.css` · `effects.css` · `fonts.css`

**Foundation cards** (`guidelines/`) — specimen cards for the Design System tab (Colors, Type, Spacing, Brand).

**Components** (`components/`) — reusable React primitives: Button, IconButton, Input, Card, Badge, Tag, Switch, Tabs, Avatar, Tooltip. Each has `.jsx` + `.d.ts` + `.prompt.md` and a directory card.

**UI kits** (`ui_kits/`)
- `arc/` — Arc browser: sidebar, command bar, spaces, web content.
- `dia/` — Dia browser: AI-native browsing with the assistant panel.
- `shadcn/` — live Tailwind v4 showcase of shadcn/ui components in the brand theme (light + dark).

**shadcn/ui theme** (`shadcn/`)
- `theme.css` — drop-in `globals.css` for any shadcn + Tailwind v4 project. Maps every shadcn token (`--background`, `--primary`, `--card`, `--ring`, `--radius`, charts, sidebar) to the Browser Company palette in `oklch`, light + dark, plus `--brand` / `bg-arc` / `text-arc` helpers.
- `README.md` — install + usage (components.json, fonts, radius, brand conventions).

> Namespace for `window.<Namespace>` is reported by `check_design_system`.
