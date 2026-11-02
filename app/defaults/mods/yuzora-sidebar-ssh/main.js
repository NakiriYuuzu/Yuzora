// The sidebar's SSH / SFTP mode: every host (~/.ssh/config, Yuzora's own,
// herdr machines) with its connection state and latency, and the selected
// host's quick actions: SFTP, a terminal, a HERDR session, known hosts.
// It also opens the dialogs connections ask through (yuzora-ssh-prompt) and
// keeps the status bar line and mode badge current. Owned by lane L4.
import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { theme, hc, run, spaces, focused, shared, setShared, openOverlay, notify } from "herdr";
import { t } from "yuzora:core";
import { hosts, hostStates, connect, disconnect, ping, prompts, claimPrompt, status, transfers, sshArgv } from "yuzora:ssh";

const I = (name) => `mods/yuzora-sidebar-ssh/icons/${name}.svg`;
const SELECTED = "yuzora.ssh.selected";
// "[code] message" from yuzora:ssh, without the code.
// Errors read "`yuzora:ssh.<fn>`: [code] text"; people see the text.
const message = (e) => String(e?.message ?? e).replace(/^(Error: )?(`[^`]*`: )?\[[a-z-]+\] /, "");

// Opens one dialog per question a connection asks, wherever it was started.
function showPrompts() {
  for (const p of prompts()) {
    if (p.claimed || !claimPrompt(p.id)) continue;
    openOverlay({ mod: "yuzora-ssh-prompt", kind: "dialog", modal: true, width: 480, props: { id: p.id } });
  }
}

export default class SidebarSsh extends View {
  init(_props, cx) {
    this.list = [];
    this.selected = shared(SELECTED) ?? null;
    this.busy = {};
    this.lastStatus = "";
    this.lastBadge = null;
    this.reload(cx);
    cx.timer.every(10000, (cx) => this.reload(cx));
    // Questions, the status line and live states: cheap calls, redrawn only on change.
    cx.timer.every(500, (cx) => {
      showPrompts();
      this.publish();
      const next = JSON.stringify(hostStates());
      if (next !== this.seen) {
        this.seen = next;
        this.list = hosts();
        cx.notify();
      }
    });
    // Latency of open sessions, now and then.
    cx.timer.every(15000, (cx) => {
      for (const h of this.list.filter((h) => h.state === "connected")) {
        cx.spawn(async (cx) => {
          await ping(h.id).catch(() => null);
          cx.notify();
        });
      }
    });
  }

  reload(cx) {
    cx.spawn(async (cx) => {
      const r = await hc(["machine", "ls", "--json"]).catch(() => null);
      let machines = null;
      try {
        if (r?.code === 0) machines = (JSON.parse(r.stdout).machines ?? []).map((m) => ({ name: m.name, target: m.target ?? null, state: m.state ?? null, temporary: m.temporary === true }));
      } catch {}
      try {
        this.list = hosts(machines ?? []);
        this.error = null;
      } catch (e) {
        this.error = message(e);
      }
      cx.notify();
    });
  }

  publish() {
    const st = status();
    const text = JSON.stringify(st);
    if (text !== this.lastStatus) {
      this.lastStatus = text;
      setShared("yuzora.status.ssh", st);
    }
    const active = transfers().filter((x) => x.state === "running" || x.state === "queued").length;
    const badge = active || null;
    if (badge !== this.lastBadge) {
      this.lastBadge = badge;
      setShared("yuzora.badge.ssh", badge);
    }
  }

  // The selected host; the first one until someone picks.
  host() {
    return this.list.find((h) => h.id === this.selected) ?? this.list[0] ?? null;
  }

  select(id, cx) {
    this.selected = id;
    setShared(SELECTED, id);
    cx.notify();
  }

  connect(h, cx, then) {
    if (this.busy[h.id]) return;
    this.busy[h.id] = true;
    cx.notify();
    cx.spawn(async (cx) => {
      try {
        await connect(h.id);
        if (then) then();
      } catch (e) {
        notify({ title: t("connect.failed", { host: h.name }), body: message(e), level: "error" });
      } finally {
        delete this.busy[h.id];
        this.list = hosts();
        cx.notify();
      }
    });
  }

  openSftp(h, cx) {
    const props = { host: h.id, name: h.name };
    if (focused()?.tab) run("pane.split", { direction: "right", mod: "yuzora-sftp", props });
    else run("tab.new", { mod: "yuzora-sftp", props, name: `SFTP · ${h.name}` });
    if (h.state !== "connected") this.connect(h, cx);
  }

  // A herdr machine opens a terminal there through herdr-core; anything else
  // runs OpenSSH in a terminal on this computer.
  terminal(h) {
    if (h.machine) {
      const space = spaces().find((s) => s.machine === h.machine && s.active) ?? spaces().find((s) => s.machine === h.machine);
      if (space) run("tab.new", { space: space.id });
      else run("space.new", { machine: h.machine });
      return;
    }
    const argv = sshArgv(h.id);
    const local = spaces().find((s) => !s.machine && s.active) ?? spaces().find((s) => !s.machine);
    if (local) run("tab.new", { space: local.id, argv, name: h.name });
    else run("space.new", { machine: "", argv, tab: h.name });
  }

  led(th, h) {
    const color = { connected: th.working, connecting: th.blocked, error: th.ansi1 ?? th.blocked }[this.busy[h.id] ? "connecting" : h.state];
    return color ? div().w(7).h(7).flex_none().rounded(4).bg(color) : div().w(7).h(7).flex_none().rounded(4).border(1.5).border_color(th.dim);
  }

  meta(h) {
    if (h.state === "connected" && h.latencyMs != null) return t("latency", { ms: h.latencyMs });
    if (this.busy[h.id] || h.state === "connecting") return t("state.connecting");
    if (h.machine) return "HERDR";
    if (h.source === "profile") return "Yuzora";
    return "";
  }

  row(th, h) {
    const sel = h.id === this.host()?.id;
    return h_flex()
      .id(`ssh-host-${h.id}`)
      .min_h(38)
      .px(8)
      .py(4)
      .gap(8)
      .items_center()
      .rounded(6)
      .cursor_pointer()
      .bg(sel ? th.hover : "#00000000")
      .hover((s) => s.bg(th.hover))
      .on_click((e, cx) => {
        this.select(h.id, cx);
        if (e.click_count === 2 && h.state !== "connected") this.connect(h, cx);
      })
      .child(this.led(th, h))
      .child(
        v_flex()
          .flex_1()
          .min_w(0)
          .child(div().truncate().font_medium().child(h.name))
          .child(div().truncate().text_size(11).text_color(th.dim).child(h.target)),
      )
      .child(div().flex_none().text_size(11).text_color(h.state === "error" ? th.blocked : th.muted).child(h.state === "error" ? t("state.error") : this.meta(h)));
  }

  action(th, id, icon, label, onClick) {
    return h_flex()
      .id(id)
      .flex_1()
      .min_w(0)
      .h(30)
      .px(8)
      .gap(6)
      .items_center()
      .justify_center()
      .rounded(6)
      .border(1)
      .border_color(th.border)
      .bg(th.popover)
      .text_size(12)
      .text_color(th.foreground)
      .cursor_pointer()
      .hover((s) => s.bg(th.raised))
      .on_click(onClick)
      .child(svg(I(icon)).size(13).text_color(th.muted))
      .child(div().truncate().child(label));
  }

  link(th, id, label, onClick, color) {
    return div().id(id).text_size(11.5).text_color(color ?? th.muted).cursor_pointer().hover((s) => s.text_color(th.foreground)).on_click(onClick).child(label);
  }

  details(th, h) {
    const busy = this.busy[h.id] || h.state === "connecting";
    const line = h.state === "connected"
      ? t("detail.connected", { kind: h.verified?.kind ?? "", ms: h.latencyMs ?? "–" })
      : busy ? t("state.connecting") : h.state === "error" ? h.error : t("detail.idle");
    return v_flex()
      .px(4)
      .gap(4)
      .child(
        h_flex()
          .gap(8)
          .items_center()
          .child(div().flex_1().min_w(0).text_size(11.5).text_color(h.state === "error" ? th.blocked : th.dim).child(line ?? ""))
          .when(h.state === "connected", (el) => el.child(this.link(th, "ssh-disconnect", t("action.disconnect"), (_e, cx) => {
            cx.spawn(async (cx) => {
              await disconnect(h.id).catch(() => null);
              this.list = hosts();
              cx.notify();
            });
          })))
          .when(h.state !== "connected" && !busy, (el) => el.child(this.link(th, "ssh-connect", t("action.connect"), (_e, cx) => this.connect(h, cx), th.foreground))),
      )
      .when(Boolean(h.verified), (el) => el.child(div().text_size(11).text_color(th.dim).truncate().child(h.verified.fingerprint)))
      .when(h.editable, (el) =>
        el.child(
          h_flex()
            .gap(10)
            .child(this.link(th, "ssh-edit", t("action.edit"), () => openOverlay({ mod: "yuzora-ssh-host", kind: "dialog", width: 460, props: { id: h.profile?.id } })))
            .child(this.link(th, "ssh-delete", t("action.delete"), () => openOverlay({ mod: "yuzora-ssh-host", kind: "dialog", width: 420, props: { id: h.profile?.id, remove: true } }))),
        ),
      );
  }

  quick(th, h) {
    return v_flex()
      .px(8)
      .pt(10)
      .pb(4)
      .gap(6)
      .flex_none()
      .child(div().px(4).text_size(11.5).text_color(th.dim).child(t("quick.title", { host: h.name })))
      .child(
        h_flex()
          .gap(6)
          .child(this.action(th, "ssh-sftp", "folder", t("quick.sftp"), (_e, cx) => this.openSftp(h, cx)))
          .child(this.action(th, "ssh-term", "terminal", t("quick.terminal"), () => this.terminal(h))),
      )
      .child(
        h_flex()
          .gap(6)
          .child(this.action(th, "ssh-herdr", "layers", t("quick.herdr"), () => openOverlay({ mod: "yuzora-ssh-herdr", kind: "dialog", width: 480, props: { host: h.id } })))
          .child(this.action(th, "ssh-known", "shield", t("quick.known"), () => openOverlay({ mod: "yuzora-ssh-known-hosts", kind: "dialog", width: 640, height: 520, props: { host: h.id } }))),
      )
      .child(this.details(th, h));
  }

  render() {
    const th = theme();
    const h = this.host();
    return v_flex()
      .id("root")
      .size_full()
      .bg(th.chrome)
      .text_color(th.foreground)
      .text_size(12.5)
      .child(
        h_flex()
          .px(12)
          .pt(10)
          .pb(8)
          .flex_none()
          .items_center()
          .child(div().flex_1().font_semibold().child(t("hosts.title")))
          .child(
            div()
              .id("ssh-add")
              .w(22)
              .h(22)
              .flex()
              .items_center()
              .justify_center()
              .rounded(5)
              .text_color(th.muted)
              .cursor_pointer()
              .hover((s) => s.bg(th.hover).text_color(th.foreground))
              .tooltip(t("hosts.add"))
              .on_click(() => openOverlay({ mod: "yuzora-ssh-host", kind: "dialog", width: 460 }))
              .child(svg(I("plus")).size(14)),
          ),
      )
      .child(
        v_flex()
          .id("ssh-hosts")
          .flex_shrink(1)
          .min_h(0)
          .px(6)
          .gap(2)
          .overflow_y_scrollbar()
          .when(Boolean(this.error), (el) => el.child(div().px(8).py(6).text_size(11.5).text_color(th.blocked).child(this.error)))
          .when(!this.list.length && !this.error, (el) =>
            el.child(v_flex().px(8).py(10).gap(4).child(div().text_size(12).text_color(th.muted).child(t("hosts.empty"))).child(div().text_size(11.5).text_color(th.dim).child(t("hosts.emptyHint")))),
          )
          .children(this.list.map((x) => this.row(th, x))),
      )
      .when(Boolean(h), (el) => el.child(this.quick(th, h)))
      .child(div().flex_1());
  }
}
