// The sidebar's Workspace mode. Owned by lane L1.
// Spaces / Agents tabs; scope chips, one per machine, lit by its connection;
// "needs attention" (agents blocked or done unseen); and the tree
// machine → Space → worktree → agent. The model comes from yuzora:shell's
// workspace(); this file only draws it. Opened as a popup with props
// { menu: "add" | "tools" } it draws that menu instead.

import { View, div, svg, image } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { spaces, panes, agents, machines, hc, exec, run, theme, shared, setShared, setAgentOrder, openOverlay, closeOverlay, props } from "herdr";
import { t, tokens } from "yuzora:core";
import { workspace } from "yuzora:shell";

const SELF = "yuzora-sidebar-workspace";
const STATUS = `mods/${SELF}/status`;
const CHROME = "mods/workspace-chrome";
const BADGE = "yuzora.badge.workspace";

function load(key, fallback) {
  try { return JSON.parse(localStorage.getItem(key) ?? "null") ?? fallback; } catch { return fallback; }
}

// An agent state's shape: dot, triangle, diamond, ring, dashed ring.
export function mark(tk, status, size = 9) {
  const st = tk.status[status] ?? tk.status.unknown;
  return svg(`${STATUS}/${tk.status[status] ? status : "unknown"}.svg`).size(size).flex_none().text_color(st.mark);
}

export default class SidebarWorkspace extends View {
  init(_props, cx) {
    if (props()?.menu) return;
    this.view = load("view", "spaces");
    this.scope = load("scope", null);
    this.folded = new Set(load("folded", []));
    this.hosts = [];
    this.host = null;
    cx.spawn(async (cx) => {
      try {
        const r = await exec("hostname", []);
        if (r.code === 0) this.host = r.stdout.trim().split(".")[0] || null;
      } catch {}
      cx.notify();
    });
    this.refreshHosts(cx);
    cx.timer.every(15000, (cx) => this.refreshHosts(cx));
  }

  // Each machine's connection (and target), for the LEDs. Herdr's machines() says only
  // whether its layout could be read.
  refreshHosts(cx) {
    cx.spawn(async (cx) => {
      try {
        const r = await hc(["machine", "ls", "--json"]);
        if (r.code === 0) this.hosts = JSON.parse(r.stdout).machines ?? [];
      } catch {}
      cx.notify();
    });
  }

  save() {
    localStorage.setItem("view", JSON.stringify(this.view));
    localStorage.setItem("scope", JSON.stringify(this.scope));
    localStorage.setItem("folded", JSON.stringify([...this.folded]));
  }

  toggle(id, cx) {
    this.folded.has(id) ? this.folded.delete(id) : this.folded.add(id);
    this.save();
    cx.notify();
  }

  machineLabel(m) {
    if (!m) return this.host ? t("machine.local", { host: this.host }) : t("machine.local_bare");
    return m;
  }

  // ---- menus (this mod opened as a popup) ---------------------------------

  menuRow(th, id, icon, label, action, enabled = true) {
    return h_flex().id(id).h(30).px(9).gap(10).rounded(6).items_center().flex_none()
      .text_color(enabled ? th.foreground : th.dim)
      .when(enabled, (el) => el.hover((s) => s.bg(th.selected)))
      .on_click(() => { if (!enabled) return; closeOverlay(props().overlay); action(); })
      .child(icon ? svg(icon).size(14).flex_none().text_color(th.muted) : div().w(14))
      .child(div().flex_1().truncate().child(label));
  }

  menu(th) {
    const which = props().menu;
    const rule = () => div().h(1).mx(4).my(4).bg(th.border);
    const space = spaces().find((s) => s.active);
    let rows;
    if (which === "add") {
      const remote = [...new Set(spaces().map((s) => s.machine).filter(Boolean))].sort();
      rows = [
        this.menuRow(th, "m-space", `${CHROME}/plus.svg`, t("menu.new_space"), () => run("space.new")),
        ...remote.map((m) => this.menuRow(th, `m-space-${m}`, `${CHROME}/server.svg`, t("menu.new_space_on", { machine: m }), () => run("space.new", { machine: m }))),
        this.menuRow(th, "m-worktree", `${CHROME}/branch.svg`, t("menu.worktree"),
          () => openOverlay({ mod: "worktree", kind: "dialog", width: 460, height: 280, props: { space: space?.id, folder: panes().find((p) => p.focused)?.cwd, action: "new" } }), Boolean(space)),
        rule(),
        this.menuRow(th, "m-machine", `${CHROME}/monitor.svg`, t("menu.machine"), () => run("mod.open", { name: "machine-add" })),
      ];
    } else {
      rows = [
        this.menuRow(th, "m-palette", `${CHROME}/search.svg`, t("tools.palette"), () => run("app.palette")),
        this.menuRow(th, "m-layout", `${CHROME}/layers.svg`, t("tools.layout"), () => run("layout.switch")),
        this.menuRow(th, "m-help", `${CHROME}/file-text.svg`, t("tools.help"), () => run("app.help")),
      ];
    }
    return v_flex().id("menu").w_full().p(5).bg(th.popover).text_size(12.5).children(rows);
  }

  // ---- the sidebar ---------------------------------------------------------

  header(th, tk, tree) {
    const tab = (id, label, count) => {
      const on = this.view === id;
      return h_flex().id(`tab-${id}`).gap(6).items_center().cursor_pointer()
        .text_color(on ? tk.text.strong : th.muted).when(on, (el) => el.font_semibold())
        .hover((s) => s.text_color(tk.text.strong))
        .on_click((_e, cx) => { this.view = id; this.save(); cx.notify(); })
        .child(label).child(div().text_size(11).font_medium().text_color(th.dim).child(String(count)));
    };
    const button = (id, icon, tip, action) => div().id(id).size(26).flex_none().flex().items_center().justify_center().rounded(5)
      .text_color(th.muted).hover((s) => s.bg(th.hover).text_color(th.foreground)).tooltip(tip).on_click(action)
      .child(svg(icon).size(14));
    const popup = (menu, height) => () => openOverlay({ mod: SELF, kind: "popup", width: 240, height, props: { menu } });
    return h_flex().h(32).flex_none().pl(8).pr(6).gap(2).items_center()
      .child(h_flex().flex_1().gap(12).items_center()
        .child(tab("spaces", t("tab.spaces"), tree.counts.spaces))
        .child(tab("agents", t("tab.agents"), tree.counts.agents)))
      .child(button("tools", `${CHROME}/layers.svg`, t("tools.title"), popup("tools", 110)))
      .child(button("add", `${CHROME}/plus.svg`, t("add"), popup("add", 180)));
  }

  led(th, tk, led) {
    if (led === "on") return div().size(7).flex_none().rounded(4).bg(tk.status.working.mark);
    if (led === "busy") return div().size(7).flex_none().rounded(4).bg(tk.status.blocked.mark);
    return div().size(7).flex_none().rounded(4).border(1.5).border_color(th.dim);
  }

  scopes(th, tk, tree) {
    const chip = (id, label, led, on, value) => h_flex().id(id).h(22).px(9).gap(5).flex_none().items_center().rounded(11)
      .border(1).border_color(on ? "#3a3b40" : th.border).bg(on ? th.selected : th.chrome)
      .text_size(11.5).text_color(on ? tk.text.strong : tk.text.soft).cursor_pointer()
      .when(!on, (el) => el.hover((s) => s.bg(th.hover)))
      .on_click((_e, cx) => { this.scope = value; this.save(); cx.notify(); })
      .when(Boolean(led), (el) => el.child(this.led(th, tk, led)))
      .child(div().max_w(120).truncate().child(label));
    return h_flex().id("scopes").h(32).flex_none().gap(4).px(8).pt(6).items_center().overflow_x_scrollbar()
      .child(chip("scope-all", t("scope.all"), null, tree.scope === null, null))
      .children(tree.scopes.map((s) => chip(`scope-${s.machine ?? "local"}`, s.machine ?? t("machine.local_bare"), s.led, s.active, s.machine ?? "")));
  }

  attention(th, tk, tree) {
    const a = tk.attention;
    return v_flex().flex_none().mx(8).mt(8).p(8).gap(2).rounded(8).bg(a.background).border(1).border_color(a.border)
      .child(div().px(2).pb(4).text_size(11.5).font_semibold().text_color(a.text).child(t("attention", { count: tree.attention.length })))
      .children(tree.attention.map((ag) => h_flex().id(`att-${ag.address}`).h(26).px(4).gap(8).items_center().rounded(5)
        .text_size(12).text_color(tk.text.strong).cursor_pointer().hover((s) => s.bg(th.hover))
        .on_click(() => run("pane.focus", { pane: ag.pane }))
        .child(mark(tk, ag.status))
        .child(div().flex_1().min_w(0).truncate().child(`${ag.name} · ${t(`status.${ag.status}`)}`))
        .child(div().flex_none().max_w(90).truncate().text_size(11).text_color(th.muted).child(ag.space_name))));
  }

  agentBadge(th, tk, ag) {
    if (!ag.brand && ag.icon) {
      return h_flex().size(16).flex_none().items_center().justify_center()
        .child(ag.tint ? svg(ag.icon).size(14).text_color(th.muted) : image(ag.icon).size(14));
    }
    return div().size(16).flex_none().rounded(4).bg(tk.agents[ag.brand] ?? tk.agents.codex).flex().items_center().justify_center()
      .text_size(9.5).font_bold().text_color(tk.text.on_badge).child([...ag.name][0]?.toUpperCase() ?? "?");
  }

  agentRow(th, tk, ag, indent, detail) {
    const st = tk.status[ag.status] ?? tk.status.unknown;
    return h_flex().id(`ag-${ag.address}`).h(26).flex_none().pl(indent).pr(6).gap(8).items_center().rounded(6)
      .when(ag.focused, (el) => el.bg(th.selected)).when(!ag.focused, (el) => el.hover((s) => s.bg(th.hover)))
      .cursor_pointer().on_click(() => run("pane.focus", { pane: ag.pane }))
      .tooltip(`${ag.name} · ${t(`status.${ag.status}`)} · ${ag.space_name}${ag.worktree ? ` · ${ag.worktree}` : ""}`)
      .child(this.agentBadge(th, tk, ag))
      .child(h_flex().flex_1().min_w(0).gap(5).items_center()
        .child(div().min_w(0).truncate().text_color(th.foreground).child(ag.name))
        .when(Boolean(detail), (el) => el.child(div().flex_1().min_w(0).truncate().text_size(11).text_color(th.dim).child(detail))))
      .child(mark(tk, ag.status))
      .child(div().flex_none().text_size(11).text_color(st.color).child(t(`status.${ag.status}`)));
  }

  machineRow(th, tk, r) {
    const label = this.machineLabel(r.machine);
    const tail = r.machine && r.led === "off" ? (r.state ?? "offline").replace(/_/g, " ") : null;
    return h_flex().id(`row-${r.id}`).h(28).flex_none().pt(6).px(6).gap(6).items_center()
      .text_size(11.5).font_semibold().text_color(th.dim).cursor_pointer()
      .hover((s) => s.text_color(th.foreground))
      .on_click((_e, cx) => this.toggle(r.id, cx))
      .child(this.led(th, tk, r.led))
      .child(div().flex_1().min_w(0).truncate().child(label))
      .when(Boolean(tail), (el) => el.child(div().flex_none().font_normal().text_size(11).child(tail)))
      .when(r.led !== "off", (el) => el.child(div().id(`new-${r.id}`).size(20).flex_none().flex().items_center().justify_center().rounded(4)
        .hover((s) => s.bg(th.hover).text_color(th.foreground)).tooltip(t("machine.add_space"))
        .on_click((_e, cx) => { cx.stop_propagation(); run("space.new", { machine: r.machine ?? "" }); })
        .child(svg(`${CHROME}/plus.svg`).size(12))))
      .child(div().size(20).flex_none().flex().items_center().justify_center().tooltip(t(r.open ? "machine.fold" : "machine.unfold", { name: label }))
        .child(svg(`${CHROME}/chevron-${r.open ? "down" : "right"}.svg`).size(11)));
  }

  spaceRow(th, tk, r) {
    const summary = [
      t(r.branches === 1 ? "space.branch" : "space.branches", { count: r.branches }),
      t(r.agents === 1 ? "space.agent" : "space.agents", { count: r.agents }),
    ].join(" · ");
    return h_flex().id(`row-${r.id}`).min_h(36).flex_none().mt(2).py(3).px(6).gap(8).items_center().rounded(6)
      .when(r.active, (el) => el.bg(th.hover)).when(!r.active, (el) => el.hover((s) => s.bg(th.hover)))
      .cursor_pointer()
      .on_click(() => run("space.select", { id: r.space }))
      .on_mouse_down("right", () => openOverlay({ mod: "menu", kind: "popup", width: 200, height: 260, props: { tab: "space", target: { id: r.space } } }))
      .child(div().size(22).flex_none().rounded(6).bg(r.color).flex().items_center().justify_center()
        .text_size(11.5).font_bold().text_color(tk.text.on_badge).child(r.initial))
      .child(v_flex().flex_1().min_w(0)
        .child(div().truncate().font_semibold().text_color(th.foreground).child(r.name))
        .child(div().truncate().text_size(11).text_color(th.dim).child(summary)))
      .when(r.blocked > 0, (el) => el.child(div().flex_none().text_size(11).text_color(tk.status.blocked.color).child(t("pending", { count: r.blocked }))))
      .child(div().id(`fold-${r.id}`).size(18).flex_none().flex().items_center().justify_center().rounded(4).text_color(th.dim)
        .hover((s) => s.bg(th.selected).text_color(th.foreground))
        .on_click((_e, cx) => { cx.stop_propagation(); this.toggle(r.id, cx); })
        .child(svg(`${CHROME}/chevron-${r.open ? "down" : "right"}.svg`).size(11)));
  }

  worktreeRow(th, tk, r) {
    const count = r.blocked > 0 ? t("pending", { count: r.blocked }) : String(r.agents);
    return h_flex().id(`row-${r.id}`).h(26).flex_none().pl(18).pr(6).gap(8).items_center().rounded(6)
      .cursor_pointer().hover((s) => s.bg(th.hover))
      .on_click((_e, cx) => this.toggle(r.id, cx))
      .child(svg(`${CHROME}/${r.branch ? "branch" : "folder"}.svg`).size(12).flex_none().text_color(th.dim))
      .child(div().flex_1().min_w(0).truncate().text_size(11.5).text_color(tk.text.soft).child(r.label || "—"))
      .child(div().flex_none().text_size(11).text_color(r.blocked > 0 ? tk.status.blocked.color : th.dim).child(count))
      .child(div().id(`wt-new-${r.id}`).size(18).flex_none().flex().items_center().justify_center().rounded(4).text_color(th.dim)
        .hover((s) => s.bg(th.selected).text_color(th.foreground)).tooltip(t("worktree.new"))
        .on_click((_e, cx) => { cx.stop_propagation(); run("tab.new", { space: r.space, folder: r.folder ?? undefined }); })
        .child(svg(`${CHROME}/plus.svg`).size(12)));
  }

  tree(th, tk, tree) {
    if (!tree.rows.some((r) => r.kind === "space")) {
      return v_flex().flex_1().min_h(0).px(6).pt(4)
        .children(tree.rows.map((r) => this.machineRow(th, tk, r)))
        .child(v_flex().px(8).pt(10).gap(8).text_size(12).text_color(th.dim)
          .child(t("empty.spaces"))
          .child(h_flex().id("empty-new").h(28).px(10).gap(6).items_center().rounded(6).border(1).border_color(th.border).bg(th.popover)
            .text_color(th.foreground).cursor_pointer().hover((s) => s.bg(th.hover))
            .on_click(() => run("space.new"))
            .child(svg(`${CHROME}/plus.svg`).size(12)).child(t("menu.new_space"))));
    }
    return v_flex().id("tree").flex_1().min_h(0).px(6).pt(4).pb(10).overflow_y_scrollbar()
      .children(tree.rows.map((r) => {
        if (r.kind === "machine") return this.machineRow(th, tk, r);
        if (r.kind === "space") return this.spaceRow(th, tk, r);
        if (r.kind === "worktree") return this.worktreeRow(th, tk, r);
        return this.agentRow(th, tk, r, 30, null);
      }));
  }

  flat(th, tk, tree) {
    if (!tree.agents.length) return div().px(14).pt(10).text_size(12).text_color(th.dim).child(t("empty.agents"));
    return v_flex().id("agents").flex_1().min_h(0).px(6).pt(6).pb(10).overflow_y_scrollbar()
      .children(tree.agents.map((ag) => this.agentRow(th, tk, ag, 8, [ag.space_name, ag.worktree].filter(Boolean).join(" · "))));
  }

  render() {
    const th = theme(), tk = tokens();
    if (props()?.menu) return this.menu(th);
    const tree = workspace({ spaces: spaces(), panes: panes(), agents: agents(), machines: machines(), hosts: this.hosts, scope: this.scope, folded: [...this.folded] });
    if (tree.scope !== this.scope) { this.scope = tree.scope; this.save(); }
    setAgentOrder(this.view === "agents" ? tree.flat_order : tree.order);
    // The mode bar's badge: everyone waiting for a reply, on every machine.
    const waiting = tree.total.blocked || null;
    if ((shared(BADGE) ?? null) !== waiting) setShared(BADGE, waiting);
    return v_flex().id("root").size_full().min_h(0).bg(th.chrome).text_color(th.foreground).text_size(12.5)
      .child(this.header(th, tk, tree))
      .child(div().h(1).flex_none().bg(th.divider ?? th.border))
      .child(this.scopes(th, tk, tree))
      .when(tree.attention.length > 0, (el) => el.child(this.attention(th, tk, tree)))
      .child(this.view === "agents" ? this.flat(th, tk, tree) : this.tree(th, tk, tree));
  }
}
