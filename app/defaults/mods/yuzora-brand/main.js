// The sidebar's top row: Yuzora's mark, clear of the macOS window buttons, and search
// (the command palette). Owned by lane L1.
import { View, div, svg, image } from "gpui-kit";
import { h_flex } from "gpui-base";
import { theme, layout, window, settings, keys, run } from "herdr";
import { t } from "yuzora:core";

// A light color, the way the app tells light themes (settings.rs is_light).
function light(hex) {
  const n = parseInt(String(hex).slice(1, 7), 16);
  return (0.2126 * ((n >> 16) & 255) + 0.7152 * ((n >> 8) & 255) + 0.0722 * (n & 255)) / 255 > 0.5;
}

// The palette's key as people read it: "⌘K", "Ctrl+Shift+P".
function shortcut(action) {
  const k = keys().find((b) => b.section === "global" && b.action === action);
  if (!k?.keys.trim()) return "";
  const mac = window()?.platform === "macos";
  const names = mac ? { ctrl: "⌃", alt: "⌥", shift: "⇧", cmd: "⌘" } : { ctrl: "Ctrl", alt: "Alt", shift: "Shift", cmd: "Super" };
  const alias = { secondary: mac ? "cmd" : "ctrl", super: "cmd", win: "cmd" };
  const stroke = k.keys.trim().split(/\s+/)[0].split("-");
  let key = stroke.pop();
  if (key === "") key = "-";
  const held = new Set(stroke.map((m) => alias[m.toLowerCase()] ?? m.toLowerCase()));
  const label = key.length === 1 ? key.toUpperCase() : key[0].toUpperCase() + key.slice(1);
  return [...Object.keys(names).filter((m) => held.has(m)).map((m) => names[m]), label].join(mac ? "" : "+");
}

export default class Brand extends View {
  render() {
    const th = theme(), w = window(), root = layout();
    // The traffic lights sit over this row when the layout draws its own title bar.
    const lights = w?.platform === "macos" && w.titlebar === "none" && !w.fullscreen && root?.traffic_lights;
    const left = lights ? Math.max(12, (root.traffic_lights[0] + 64) / (settings().zoom || 1)) : 12;
    const key = shortcut("app.palette");
    return h_flex().id("root").size_full().pl(left).pr(8).gap(8).items_center().bg(th.chrome)
      // In full color (image, not svg: svg tints a one-color mask).
      .child(image(`mods/yuzora-brand/${light(th.chrome) ? "mark-light" : "mark"}.svg`).w(18).h(18).flex_none())
      .child(div().flex_1().text_size(13.5).font_bold().text_color(th.foreground).child(t("name")))
      .child(div().id("search").size(26).flex_none().flex().items_center().justify_center().rounded(6).text_color(th.muted)
        .hover((s) => s.bg(th.hover).text_color(th.foreground))
        .tooltip(key ? t("search_key", { key }) : t("search"))
        .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
        .on_click(() => run("app.palette"))
        .child(svg("mods/workspace-chrome/search.svg").size(14)));
  }
}
