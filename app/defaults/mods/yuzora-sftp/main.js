// SFTP: this computer and a host side by side, each browsed on its own.
// → / ← (or dragging rows across, or dropping files from the desktop on the
// host's side) queue transfers; the queue shows progress, speed, resume,
// cancel and retry. Double-clicking a host's file opens it in a text editor
// and uploads every save. Opened with props { host, name } (the SSH
// sidebar's Open SFTP, or the new-pane menu, which asks for a host). Owned by lane L4.
import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { theme, props, exec, window, openOverlay, notify, shared, setShared } from "herdr";
import { t, getSetting, setSetting } from "yuzora:core";
import {
  hosts, connect, list, mkdir, rename, remove, transfer, transfers, cancelTransfer, retryTransfer, clearTransfers,
  openRemote, openInNotepad, edits, stopEdit, overwriteEdit, prompts, claimPrompt, localHome, status,
} from "yuzora:ssh";

const I = (name) => `mods/yuzora-sidebar-ssh/icons/${name}.svg`;
// Errors read "`yuzora:ssh.<fn>`: [code] text"; people see the text.
const message = (e) => String(e?.message ?? e).replace(/^(Error: )?(`[^`]*`: )?\[[a-z-]+\] /, "");
const LOCAL = "local";

// Plain data xdg-open may have (its handler is an editor), and names it never gets
// whatever their content looks like (a browser, a launcher or an interpreter would run them).
const PLAIN = new Set(["text/plain", "text/markdown", "text/csv", "text/x-log", "application/json", "application/toml", "application/yaml", "application/x-yaml", "inode/x-empty"]);
const RUNNABLE = /\.(desktop|sh|bash|zsh|fish|py|pl|rb|php|js|mjs|html?|xhtml|xml|xsl|xslt|svg|jar|appimage|run|bin|exe|msi|bat|cmd|com|ps1|vbs|lnk|deb|rpm|flatpakref)$/i;

// A text editor, never the file's default app: a host's .bat, .terminal, .desktop or
// .jar must not run here. xdg-open goes by type, so on Linux it only gets plain data.
async function openInEditor(path) {
  const platform = window()?.platform;
  if (platform === "macos") return exec("open", ["-t", path]).catch(() => null);
  // Not `exec`: it waits for the program and stops it after 30 s, Notepad's window with it.
  if (platform === "windows") return Promise.resolve().then(() => openInNotepad(path)).catch(() => null);
  const mime = (await exec("xdg-mime", ["query", "filetype", path]).catch(() => null))?.stdout?.trim() ?? "";
  if (PLAIN.has(mime) && !RUNNABLE.test(path)) return exec("xdg-open", [path]).catch(() => null);
  notify({ title: t("edit.openYourself", { path }) });
}

// The status line and mode badge, set only when they change (the sidebar does the same:
// whichever of the two is open keeps them current).
function publish() {
  const active = transfers().filter((x) => x.state === "running" || x.state === "queued").length;
  for (const [key, value] of [["yuzora.status.ssh", status()], ["yuzora.badge.ssh", active || null]]) {
    if (JSON.stringify(shared(key) ?? null) !== JSON.stringify(value)) setShared(key, value);
  }
}

export function size(n) {
  if (n == null) return "";
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024, i = 0;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  return `${v >= 100 ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
}

// "184 / 296 MB": both in the total's unit.
function progress(done, total) {
  if (!total) return size(done);
  const units = ["B", "KB", "MB", "GB", "TB"];
  let i = 0, d = 1;
  while (total / d >= 1024 && i < units.length - 1) { d *= 1024; i++; }
  const f = (n) => (n / d >= 100 || i === 0 ? Math.round(n / d) : (n / d).toFixed(1));
  return `${f(done)} / ${f(total)} ${units[i]}`;
}

function showPrompts() {
  for (const p of prompts()) {
    if (p.claimed || !claimPrompt(p.id)) continue;
    openOverlay({ mod: "yuzora-ssh-prompt", kind: "dialog", modal: true, width: 480, props: { id: p.id } });
  }
}

export default class Sftp extends View {
  init(_props, cx) {
    this.host = props()?.host ?? null;
    this.name = props()?.name ?? this.host ?? "";
    this.sides = {
      [LOCAL]: { path: localHome(), listing: null, error: null, loading: false, selected: [], anchor: null },
      remote: { path: "~", listing: null, error: null, loading: false, selected: [], anchor: null },
    };
    this.hidden = false;
    this.preserve = (getSetting("ssh") ?? {}).preserve !== false;
    this.state = "connecting";
    this.drag = null;
    this.edit = null; // { side, mode: "mkdir" | "rename", path, input }
    this.confirm = null; // { text, action }
    this.finished = new Set(transfers().filter((x) => !["queued", "running"].includes(x.state)).map((x) => x.id));
    this.load(LOCAL, null, cx);
    if (this.host) this.open(cx);
    cx.timer.every(500, (cx) => this.tick(cx));
  }

  open(cx) {
    this.state = "connecting";
    this.sides.remote.error = null;
    cx.notify();
    cx.spawn(async (cx) => {
      try {
        const h = await connect(this.host);
        this.name = h?.name ?? this.name;
        this.id = h?.id ?? null;
        this.state = "connected";
        this.load("remote", "~", cx);
      } catch (e) {
        this.state = "error";
        this.sides.remote.error = message(e);
      }
      cx.notify();
    });
  }

  // Questions, transfer progress, and listings to refresh when a transfer lands.
  tick(cx) {
    showPrompts();
    publish();
    const all = transfers().filter((x) => x.host === this.hostId());
    let changed = all.some((x) => x.state === "running" || x.state === "queued");
    for (const x of all) {
      if (["queued", "running"].includes(x.state) || this.finished.has(x.id)) continue;
      this.finished.add(x.id);
      changed = true;
      if (x.state === "done") this.load(x.direction === "upload" ? "remote" : LOCAL, null, cx);
    }
    const e = JSON.stringify(edits().filter((x) => x.host === this.hostId()).map((x) => [x.id, x.state, x.uploads]));
    if (e !== this.seenEdits) {
      this.seenEdits = e;
      changed = true;
    }
    if (changed) cx.notify();
  }

  // The host's id, as transfers and edits name it (looked up once).
  hostId() {
    this.id ??= hosts().find((h) => h.id === this.host || h.name === this.host)?.id ?? null;
    return this.id ?? this.host;
  }

  load(which, path, cx) {
    const side = this.sides[which];
    const target = path ?? side.path;
    side.loading = true;
    cx.notify();
    cx.spawn(async (cx) => {
      try {
        side.listing = await list(which === LOCAL ? null : this.host, target);
        if (side.path !== side.listing.path) side.selected = [];
        side.path = side.listing.path;
        side.error = null;
      } catch (e) {
        side.error = message(e);
      } finally {
        side.loading = false;
        cx.notify();
      }
    });
  }

  entries(which) {
    const l = this.sides[which].listing;
    return (l?.entries ?? []).filter((e) => this.hidden || !e.hidden);
  }

  click(which, entry, e, cx) {
    const side = this.sides[which];
    if (e.click_count === 2) {
      if (entry.dir) this.load(which, entry.path, cx);
      else if (which !== LOCAL) this.editRemote(entry, cx);
      return;
    }
    if (e.modifiers?.platform || e.modifiers?.control) {
      side.selected = side.selected.includes(entry.path) ? side.selected.filter((p) => p !== entry.path) : [...side.selected, entry.path];
    } else if (e.modifiers?.shift && side.anchor) {
      const names = this.entries(which).map((x) => x.path);
      const [a, b] = [names.indexOf(side.anchor), names.indexOf(entry.path)].sort((x, y) => x - y);
      side.selected = a < 0 ? [entry.path] : names.slice(a, b + 1);
    } else side.selected = [entry.path];
    side.anchor = entry.path;
    cx.notify();
  }

  // Copies the selection (or `paths`) from one side into the other side's folder.
  send(from, cx, paths) {
    const to = from === LOCAL ? "remote" : LOCAL;
    const chosen = paths ?? this.sides[from].selected;
    const dest = this.sides[to];
    if (!chosen.length || !dest.listing || !this.host) return;
    const names = new Set(dest.listing.entries.map((e) => e.name));
    const clash = chosen.map((p) => p.split(/[\\/]/).filter(Boolean).pop()).filter((n) => names.has(n));
    const go = (cx) => {
      for (const p of chosen) {
        try {
          transfer({ host: this.host, direction: from === LOCAL ? "upload" : "download", from: p, toDir: dest.path, preserve: this.preserve });
        } catch (e) {
          notify({ title: t("transfer.failed"), body: message(e), level: "error" });
        }
      }
      this.confirm = null;
      cx.notify();
    };
    if (clash.length) this.confirm = { text: t("confirm.overwrite", { names: clash.join(", ") }), label: t("confirm.overwriteButton"), action: go };
    else go(cx);
    cx.notify();
  }

  editRemote(entry, cx) {
    cx.spawn(async (cx) => {
      try {
        const ed = await openRemote(this.host, entry.path);
        await openInEditor(ed.local);
      } catch (e) {
        notify({ title: t("edit.failed", { name: entry.name }), body: message(e), level: "error" });
      }
      cx.notify();
    });
  }

  startEdit(which, mode, cx) {
    const side = this.sides[which];
    const path = side.selected[0];
    const current = mode === "rename" ? (path ?? "").split(/[\\/]/).filter(Boolean).pop() ?? "" : "";
    if (mode === "rename" && !path) return;
    const input = InputState.new({ placeholder: mode === "mkdir" ? t("edit.folderName") : "", value: current });
    input.on("submit", (_e, cx) => this.finishEdit(cx));
    this.edit = { side: which, mode, path, input };
    input.focus();
    cx.notify();
  }

  finishEdit(cx) {
    const ed = this.edit;
    if (!ed) return;
    const name = ed.input.value().trim();
    this.edit = null;
    if (!name) return cx.notify();
    const side = ed.side === LOCAL ? null : this.host;
    cx.spawn(async (cx) => {
      try {
        if (ed.mode === "mkdir") await mkdir(side, this.sides[ed.side].path, name);
        else await rename(side, ed.path, name);
      } catch (e) {
        notify({ title: t(ed.mode === "mkdir" ? "edit.mkdirFailed" : "edit.renameFailed"), body: message(e), level: "error" });
      }
      this.load(ed.side, null, cx);
    });
  }

  askDelete(which, cx) {
    const chosen = this.sides[which].selected;
    if (!chosen.length) return;
    this.confirm = {
      text: t(chosen.length === 1 ? "confirm.deleteOne" : "confirm.deleteMany", { count: chosen.length, name: chosen[0].split(/[\\/]/).filter(Boolean).pop() }),
      label: t("confirm.deleteButton"),
      danger: true,
      action: (cx) => {
        this.confirm = null;
        cx.spawn(async (cx) => {
          for (const p of chosen) {
            try {
              await remove(which === LOCAL ? null : this.host, p);
            } catch (e) {
              notify({ title: t("confirm.deleteFailed"), body: message(e), level: "error" });
              break;
            }
          }
          this.sides[which].selected = [];
          this.load(which, null, cx);
        });
      },
    };
    cx.notify();
  }

  icon(th, id, name, tip, onClick, enabled = true) {
    return div()
      .id(id)
      .w(22)
      .h(22)
      .flex()
      .flex_none()
      .items_center()
      .justify_center()
      .rounded(5)
      .text_color(enabled ? th.muted : th.dim)
      .when(!enabled, (el) => el.opacity(0.45))
      .when(enabled, (el) => el.cursor_pointer().hover((s) => s.bg(th.hover).text_color(th.foreground)).on_click(onClick))
      .tooltip(tip)
      .child(svg(I(name)).size(13));
  }

  column(th, which) {
    const side = this.sides[which];
    const remote = which !== LOCAL;
    const led = remote ? { connected: th.working, connecting: th.blocked, error: th.ansi1 ?? th.blocked }[this.state] : th.working;
    const rows = this.entries(which);
    const editing = this.edit?.side === which ? this.edit : null;
    const selected = side.selected.length > 0;
    const dropTarget = this.drag && this.drag.side !== which;
    return v_flex()
      .id(`sftp-col-${which}`)
      .flex_1()
      .min_w(0)
      .min_h(0)
      .when(Boolean(dropTarget), (el) => el.bg(th.hover))
      .on_mouse_up("left", (_e, cx) => {
        const d = this.drag;
        this.drag = null;
        if (d && d.side !== which) this.send(d.side, cx, d.paths);
        else cx.notify();
      })
      .when(remote, (el) => el.on_drop((e, cx) => this.send(LOCAL, cx, e.paths)))
      .child(
        h_flex()
          .h(28)
          .flex_none()
          .px(10)
          .gap(6)
          .items_center()
          .border_b(1)
          .border_color(th.divider ?? th.border)
          .text_size(12)
          .child(div().w(6).h(6).flex_none().rounded(3).bg(led))
          .child(div().flex_none().font_semibold().child(remote ? this.name : t("local")))
          .child(div().flex_1().min_w(0).truncate().text_size(11.5).text_color(th.muted).child(side.path ?? ""))
          .child(this.icon(th, `sftp-home-${which}`, "home", t("tip.home"), (_e, cx) => this.load(which, remote ? "~" : localHome(), cx)))
          .child(this.icon(th, `sftp-up-${which}`, "parent", t("tip.parent"), (_e, cx) => this.load(which, side.listing?.parent, cx), Boolean(side.listing?.parent)))
          .child(this.icon(th, `sftp-reload-${which}`, "refresh", t("tip.refresh"), (_e, cx) => this.load(which, null, cx)))
          .child(this.icon(th, `sftp-mkdir-${which}`, "folder-plus", t("tip.mkdir"), (_e, cx) => this.startEdit(which, "mkdir", cx), Boolean(side.listing)))
          .child(this.icon(th, `sftp-rename-${which}`, "pencil", t("tip.rename"), (_e, cx) => this.startEdit(which, "rename", cx), side.selected.length === 1))
          .child(this.icon(th, `sftp-delete-${which}`, "trash", t("tip.delete"), (_e, cx) => this.askDelete(which, cx), selected)),
      )
      .when(Boolean(editing), (el) =>
        el.child(
          h_flex()
            .h(30)
            .px(10)
            .gap(6)
            .items_center()
            .border_b(1)
            .border_color(th.divider ?? th.border)
            .child(svg(I(editing.mode === "mkdir" ? "folder-plus" : "pencil")).size(13).text_color(th.muted))
            .child(Input.new(editing.input).flex_1().h(24).px(8).items_center().rounded(5).border(1).border_color(th.input).bg(th.background).text_size(12).text_color(th.foreground))
            .child(this.icon(th, `sftp-edit-cancel-${which}`, "x", t("cancel"), (_e, cx) => { this.edit = null; cx.notify(); })),
        ),
      )
      .child(
        v_flex()
          .id(`sftp-list-${which}`)
          .flex_1()
          .min_h(0)
          .overflow_y_scrollbar()
          .when(Boolean(side.error), (el) =>
            el.child(
              v_flex()
                .p(12)
                .gap(8)
                .child(div().text_size(12).text_color(th.blocked).child(side.error))
                .when(remote, (el) => el.child(div().id("sftp-retry-connect").text_size(12).text_color(th.foreground).cursor_pointer().hover((s) => s.opacity(0.8)).on_click((_e, cx) => this.open(cx)).child(t("retry")))),
            ),
          )
          .when(!side.error && !side.listing, (el) => el.child(div().p(12).text_size(12).text_color(th.dim).child(remote && this.state === "connecting" ? t("connecting", { host: this.name }) : t("loading"))))
          .when(Boolean(side.listing) && rows.length === 0 && !side.error, (el) => el.child(div().p(12).text_size(12).text_color(th.dim).child(t("empty"))))
          .children(rows.map((e) => this.row(th, which, e))),
      );
  }

  row(th, which, e) {
    const sel = this.sides[which].selected.includes(e.path);
    const color = e.dir ? th.dim : e.kind === "link" ? th.done : e.mode != null && (e.mode & 0o111) ? th.working : th.muted;
    return h_flex()
      .id(`sftp-row-${which}-${e.path}`)
      .h(27)
      .flex_none()
      .px(12)
      .gap(8)
      .items_center()
      .border_b(1)
      .border_color(th.canvas ?? th.border)
      .bg(sel ? th.selected : "#00000000")
      .hover((s) => s.bg(sel ? th.selected : th.hover))
      .cursor_pointer()
      .on_mouse_down("left", (ev, cx) => {
        if (ev.click_count > 1) return;
        const side = this.sides[which];
        this.drag = { side: which, paths: side.selected.includes(e.path) ? side.selected : [e.path] };
      })
      .on_click((ev, cx) => this.click(which, e, ev, cx))
      .child(svg(I(e.dir ? "folder" : e.kind === "link" ? "link" : "file")).size(13).flex_none().text_color(color))
      .child(div().flex_1().min_w(0).truncate().text_size(12).text_color(e.hidden ? th.muted : th.foreground).child(e.name))
      .child(div().w(80).flex_none().text_size(11).text_color(th.muted).child(e.dir ? "—" : size(e.size)));
  }

  arrows(th) {
    const can = (from) => this.state === "connected" && this.sides[from].selected.length > 0 && Boolean(this.sides[from === LOCAL ? "remote" : LOCAL].listing);
    const btn = (id, label, from, solid) =>
      div()
        .id(id)
        .w(28)
        .h(28)
        .flex()
        .items_center()
        .justify_center()
        .rounded(6)
        .font_bold()
        .when(solid, (el) => el.bg(th.solid).text_color(th.solid_foreground))
        .when(!solid, (el) => el.border(1).border_color(th.input).text_color(th.foreground))
        .when(!can(from), (el) => el.opacity(0.35))
        .when(can(from), (el) => el.cursor_pointer().hover((s) => s.opacity(0.85)).on_click((_e, cx) => this.send(from, cx)))
        .tooltip(t(from === LOCAL ? "tip.upload" : "tip.download"))
        .child(label);
    return v_flex()
      .w(40)
      .flex_none()
      .gap(8)
      .items_center()
      .justify_center()
      .border_l(1)
      .border_r(1)
      .border_color(th.divider ?? th.border)
      .child(btn("sftp-upload", "→", LOCAL, true))
      .child(btn("sftp-download", "←", "remote", false));
  }

  transferRow(th, x) {
    const pct = x.total ? Math.min(100, Math.round((x.done / x.total) * 100)) : x.state === "done" ? 100 : 0;
    const running = x.state === "running" || x.state === "queued";
    const tone = { done: th.working, failed: th.blocked, cancelled: th.dim }[x.state] ?? th.done;
    const bits = [progress(x.done, x.total)];
    if (x.resumed) bits.push(t("transfer.resumed"));
    if (x.state === "running" && x.rate) bits.push(`${size(x.rate)}/s`);
    if (x.state === "queued") bits.push(t("transfer.queued"));
    if (x.state === "cancelled") bits.push(t("transfer.cancelled"));
    if (x.files > 1) bits.push(t("transfer.files", { done: x.filesDone, count: x.files }));
    return v_flex()
      .flex_none()
      .child(
        h_flex()
          .h(32)
          .px(12)
          .gap(10)
          .items_center()
          .text_size(12)
          .child(div().flex_none().font_bold().text_color(tone).child(x.direction === "upload" ? "↑" : "↓"))
          .child(div().w(180).flex_none().truncate().text_size(11.5).child(x.name))
          .child(div().flex_1().h(5).rounded(3).bg(th.raised).child(div().h(5).rounded(3).w(`${pct}%`).bg(tone)))
          .child(div().flex_none().text_size(11).text_color(th.muted).child(bits.join(" · ")))
          .when(running, (el) => el.child(this.icon(th, `sftp-cancel-${x.id}`, "x", t("transfer.cancel"), () => cancelTransfer(x.id))))
          .when(x.state === "failed" || x.state === "cancelled", (el) => el.child(this.icon(th, `sftp-retry-${x.id}`, "refresh", t("transfer.retry"), (_e, cx) => {
            try { retryTransfer(x.id); } catch (e) { notify({ title: t("transfer.failed"), body: message(e), level: "error" }); }
            cx.notify();
          }))),
      )
      .when(x.state === "failed" && Boolean(x.error), (el) => el.child(div().px(34).pb(6).text_size(11).text_color(th.blocked).truncate().child(x.error)));
  }

  editRow(th, ed) {
    const name = ed.remote.split("/").pop();
    const text = ed.state === "conflict" ? ed.error : ed.state === "error" ? ed.error : ed.uploads ? t("edit.uploaded", { count: ed.uploads }) : t("edit.watching");
    return h_flex()
      .h(28)
      .px(12)
      .gap(10)
      .items_center()
      .text_size(11.5)
      .child(svg(I("pencil")).size(12).text_color(ed.state === "conflict" || ed.state === "error" ? th.blocked : th.muted))
      .child(div().w(180).flex_none().truncate().child(name))
      .child(div().flex_1().min_w(0).truncate().text_color(ed.state === "conflict" || ed.state === "error" ? th.blocked : th.dim).child(text ?? ""))
      .when(ed.state === "conflict", (el) => el.child(div().id(`sftp-edit-over-${ed.id}`).text_color(th.foreground).cursor_pointer().on_click(() => overwriteEdit(ed.id)).child(t("edit.overwrite"))))
      .child(this.icon(th, `sftp-edit-stop-${ed.id}`, "x", t("edit.stop"), (_e, cx) => { stopEdit(ed.id); cx.notify(); }));
  }

  footer(th) {
    const mine = transfers().filter((x) => x.host === this.hostId());
    const shown = [...mine.filter((x) => x.state === "running"), ...mine.filter((x) => x.state === "queued"), ...mine.filter((x) => !["running", "queued"].includes(x.state)).reverse()].slice(0, 4);
    const open = edits().filter((x) => x.host === this.hostId() && x.state !== "stopped");
    const finished = mine.some((x) => !["running", "queued"].includes(x.state));
    if (!shown.length && !open.length) return null;
    return v_flex()
      .flex_none()
      .border_t(1)
      .border_color(th.divider ?? th.border)
      .children(shown.map((x) => this.transferRow(th, x)))
      .children(open.map((x) => this.editRow(th, x)))
      .when(finished, (el) =>
        el.child(h_flex().h(22).px(12).justify_end().child(div().id("sftp-clear").text_size(11).text_color(th.dim).cursor_pointer().hover((s) => s.text_color(th.foreground)).on_click((_e, cx) => { clearTransfers(); cx.notify(); }).child(t("transfer.clear")))),
      );
  }

  toolbar(th) {
    const running = transfers().filter((x) => x.host === this.hostId() && x.state === "running");
    const done = running.reduce((n, x) => n + x.done, 0), total = running.reduce((n, x) => n + x.total, 0);
    const chip = (id, on, label, toggle) =>
      div()
        .id(id)
        .h(20)
        .px(7)
        .flex()
        .items_center()
        .rounded(10)
        .border(1)
        .border_color(on ? th.ring ?? th.input : th.border)
        .text_size(11)
        .text_color(on ? th.foreground : th.dim)
        .cursor_pointer()
        .hover((s) => s.bg(th.hover))
        .on_click(toggle)
        .child(label);
    return h_flex()
      .h(28)
      .flex_none()
      .px(8)
      .gap(7)
      .items_center()
      .border_b(1)
      .border_color(th.divider ?? th.border)
      .text_size(12)
      .child(svg(I("folder")).size(13).text_color(th.muted))
      .child(div().font_semibold().child("SFTP"))
      .child(div().text_color(th.dim).truncate().child(t("between", { host: this.name })))
      .child(div().flex_1())
      .when(running.length > 0, (el) => el.child(div().text_size(11.5).text_color(th.done).child(t("running", { count: running.length, pct: total ? Math.round((done / total) * 100) : 0 }))))
      .child(chip("sftp-hidden", this.hidden, t("hidden"), (_e, cx) => { this.hidden = !this.hidden; cx.notify(); }))
      .child(chip("sftp-preserve", this.preserve, t("preserve"), (_e, cx) => {
        this.preserve = !this.preserve;
        setSetting("ssh", { ...(getSetting("ssh") ?? {}), preserve: this.preserve });
        cx.notify();
      }));
  }

  render() {
    const th = theme();
    if (!this.host) {
      return v_flex().id("root").size_full().p(16).gap(6).bg(th.background).text_color(th.foreground)
        .child(div().font_semibold().child("SFTP"))
        .child(div().pb(6).text_size(12).text_color(th.muted).child(t("noHost")))
        .children(hosts().map((h) =>
          h_flex().id(`sftp-pick-${h.id}`).h(32).px(8).gap(8).items_center().rounded(6).cursor_pointer().hover((s) => s.bg(th.hover))
            .on_click((_e, cx) => { this.host = h.id; this.name = h.name; this.id = h.id; this.open(cx); })
            .child(svg(I("server")).size(13).text_color(th.muted))
            .child(div().font_medium().child(h.name))
            .child(div().flex_1().min_w(0).truncate().text_size(11.5).text_color(th.dim).child(h.target))));
    }
    const confirm = this.confirm;
    return v_flex()
      .id("root")
      .size_full()
      .bg(th.background)
      .text_color(th.foreground)
      .text_size(12.5)
      .on_mouse_up("left", (_e, cx) => {
        if (this.drag) {
          this.drag = null;
          cx.notify();
        }
      })
      .child(this.toolbar(th))
      .when(Boolean(confirm), (el) =>
        el.child(
          h_flex()
            .h(34)
            .px(12)
            .gap(10)
            .items_center()
            .bg(th.raised)
            .border_b(1)
            .border_color(th.border)
            .child(div().flex_1().min_w(0).truncate().text_size(12).child(confirm.text))
            .child(div().id("sftp-confirm-no").text_size(12).text_color(th.muted).cursor_pointer().on_click((_e, cx) => { this.confirm = null; cx.notify(); }).child(t("cancel")))
            .child(div().id("sftp-confirm-yes").h(24).px(10).flex().items_center().rounded(5).text_size(12).font_medium().cursor_pointer()
              .when(Boolean(confirm.danger), (el) => el.border(1).border_color(th.blocked).text_color(th.blocked))
              .when(!confirm.danger, (el) => el.bg(th.solid).text_color(th.solid_foreground))
              // A confirm runs later, on its own click: that click's cx, not the one that asked.
              .on_click((_e, cx) => confirm.action(cx))
              .child(confirm.label)),
        ),
      )
      .child(h_flex().flex_1().min_h(0).items_stretch().child(this.column(th, LOCAL)).child(this.arrows(th)).child(this.column(th, "remote")))
      .child(this.footer(th) ?? div());
  }
}
