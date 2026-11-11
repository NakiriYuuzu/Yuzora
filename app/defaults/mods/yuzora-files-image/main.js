// An image or SVG in a pane: props {path, on?}. Fit to the pane or actual size;
// a file that changes on disk is shown again. Owned by lane L5.
import { View, div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { theme, watchFolder, home, props } from "herdr";
import { t } from "yuzora:core";
import { stat, ImageView } from "yuzora:files";

// Paths: a Windows one (`C:\…`, `\\server\…`) takes either slash and ignores case;
// anywhere else only "/" separates and case counts. (A test runs these on their own:
// they use nothing else from this file.)
function isWin(path) {
  return /^([A-Za-z]:[\\/]|\\\\)/.test(path);
}

// The folder holding `path`: "" for a file at the top, `C:\` for one at a drive's top.
function parent(path) {
  const at = isWin(path) ? Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\")) : path.lastIndexOf("/");
  if (at < 0) return "";
  return at === 2 && path[1] === ":" ? path.slice(0, 3) : path.slice(0, at);
}

function isRoot(dir) {
  return dir === "/" || /^[A-Za-z]:[\\/]?$/.test(dir);
}

function baseName(path) {
  return path.split(isWin(path) ? /[\\/]/ : "/").pop();
}

// The same file or folder: what a watch reports against what this pane opened.
function samePath(a, b) {
  if (!isWin(a) && !isWin(b)) return a === b;
  const plain = p => p.replace(/\\/g, "/").toLowerCase().replace(/(?<=.)\/$/, "");
  return plain(a) === plain(b);
}

export default class ImagePane extends View {
  init(_p, cx) {
    const p = props() ?? {};
    this.path = p.path ?? null;
    this.on = p.on ?? null;
    this.fit = localStorage.getItem("fit") ?? "contain";
    this.checker = localStorage.getItem("checker") === "1";
    this.rev = 0;
    this.stamp = null;
    if (!this.path) return;
    this.probe(cx);
    // Watches are recursive on macOS: an image right in the home folder (or /) is polled only.
    const dir = parent(this.path) || "/";
    if (!this.on && !isRoot(dir) && !samePath(dir, home())) {
      cx.spawn(async cx => {
        try {
          for (;;) {
            const changed = await watchFolder(dir);
            // The file itself changed: shown again even if its size and time did not.
            if (changed.some(p => samePath(p, this.path))) { this.rev++; this.probe(cx); }
            else if (changed.some(p => samePath(p, dir))) this.probe(cx);
          }
        } catch {}
      });
    }
    cx.timer.every(this.on ? 10000 : 30000, cx => this.probe(cx));
  }

  // Shows the file again when its size or time changed. Another machine
  // reports whole seconds: a rewrite in the same second keeps the stamp, so
  // a new stamp there is read once more after that second is over.
  probe(cx) {
    cx.spawn(async cx => {
      const s = await stat(this.path, { on: this.on }).catch(() => null);
      const key = s ? `${s.len}:${s.mtime_ms}` : "missing";
      if (key !== this.stamp) {
        if (this.stamp !== null) this.rev++;
        if (this.on && s) cx.timer.after(2500, cx => { this.rev++; cx.notify(); });
      }
      this.size = s?.len ?? null;
      this.stamp = key;
      cx.notify();
    });
  }

  render(cx) {
    const th = theme();
    const name = this.path ? baseName(this.path) : "";
    const toggle = (id, label, on, onClick) => div().id(id).h(18).px(6).rounded(3).flex().items_center().text_size(11).cursor_pointer()
      .bg(on ? th.input : "#00000000").text_color(on ? th.foreground : th.muted).on_click(onClick).child(label);
    const size = this.size == null ? "" : this.size < 1024 ? `${this.size} B` : this.size < 1048576 ? `${(this.size / 1024).toFixed(1)} KB` : `${(this.size / 1048576).toFixed(1)} MB`;
    return v_flex().id("root").size_full().min_h(0).bg(th.background).text_color(th.foreground).text_size(12.5)
      .child(h_flex().h(28).flex_none().gap(7).pl(8).pr(6).items_center().text_size(12).border_b(1).border_color(th.border)
        .child(div().flex_none().text_size(10).font_bold().text_color(th.ansi5).child(name.toLowerCase().endsWith(".svg") ? "SVG" : "IMG"))
        .child(div().flex_none().max_w(260).truncate().font_semibold().child(name))
        .child(div().flex_1().min_w(0).truncate().text_color(th.dim).child(this.stamp === "missing" ? t("missing") : size))
        .child(h_flex().flex_none().p(2).rounded(5).bg(th.raised)
          .child(toggle("fit", t("fit"), this.fit === "contain", (_e, cx) => { this.fit = "contain"; localStorage.setItem("fit", "contain"); cx.notify(); }))
          .child(toggle("actual", t("actual"), this.fit === "actual", (_e, cx) => { this.fit = "actual"; localStorage.setItem("fit", "actual"); cx.notify(); })))
        .child(toggle("checker", t("checker"), this.checker, (_e, cx) => { this.checker = !this.checker; localStorage.setItem("checker", this.checker ? "1" : "0"); cx.notify(); })))
      .child(div().flex_1().min_h(0).child(this.path
        ? ImageView.new("image", { path: this.path, on: this.on, rev: this.rev, fit: this.fit, colors: this.checker ? { muted: th.solid_foreground, checker: th.solid } : { muted: th.dim } })
        : div().p(24).text_color(th.dim).child(t("none"))));
  }
}
