// The push dialog (IntelliJ's): where the branch goes, the commits it sends,
// force with lease, tags, set upstream. Props {cwd, on}.
import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { props, closeOverlay, theme, notify } from "herdr";
import { t } from "yuzora:core";
import { open, pushPreview, push, cancel, settings as gitSettings } from "yuzora:git";

const ago = (s) => {
  const d = Date.now() / 1000 - s;
  return d < 3600 ? `${Math.max(1, Math.round(d / 60))}m` : d < 86400 ? `${Math.round(d / 3600)}h` : `${Math.round(d / 86400)}d`;
};

export default class GitPush extends View {
  init(_props, cx) {
    const s = gitSettings();
    this.preview = null; this.error = null; this.busy = false; this.note = null;
    this.force = ""; this.tags = s.pushTags ?? "none"; this.noVerify = false; this.confirmForce = false;
    this.target = InputState.new({ placeholder: "" });
    this.target.on("change", (_e, cx) => cx.notify());
    this.target.on("submit", (_e, cx) => this.push(cx));
    this.handle = cx.focus_handle();
    this.handle.focus();
    const p = props() ?? {};
    if (!p.cwd) { this.error = t("noRepo"); return; }
    cx.spawn(async (cx) => {
      try {
        this.repo = await open(p.cwd, p.on ?? null);
        this.preview = await pushPreview(this.repo.id);
        this.remote = this.preview.remote;
        this.target.set_value(this.preview.target);
      } catch (e) { this.error = String(e); }
      cx.notify();
    });
  }

  close() { if (this.token) cancel(this.token); closeOverlay(props()?.overlay); }

  push(cx) {
    const p = this.preview;
    if (!p || this.busy) return;
    if (this.force && !this.confirmForce) { this.confirmForce = true; cx.notify(); return; }
    this.busy = true; this.note = null; this.token = `push-${Date.now()}`;
    const target = this.target.value().trim() || p.target;
    cx.spawn(async (cx) => {
      try {
        const out = await push(this.repo.id, { remote: this.remote, branch: p.branch, target, force: this.force, tags: this.tags, noVerify: this.noVerify, setUpstream: p.newBranch, token: this.token });
        if (out.ok === false) {
          this.note = out.rejected ? t("rejected") : out.output || t("failed");
          this.confirmForce = false;
          return;
        }
        notify({ title: out.updated ? t("doneUpdated") : t("done"), body: `${p.branch} → ${this.remote}/${target}`, level: "success", timeout: 3000 });
        this.token = null;
        closeOverlay(props()?.overlay);
      } catch (e) { this.note = String(e); }
      finally { this.busy = false; this.token = null; cx.notify(); }
    });
    cx.notify();
  }

  key(e, cx) {
    if (e.key === "escape") { this.close(); cx.stop_propagation(); }
    else if (e.key === "enter" && (e.modifiers?.platform || e.modifiers?.control)) { this.push(cx); cx.stop_propagation(); }
  }

  choice(id, label, on, th, action) {
    return div().id(id).h(22).px(8).flex().items_center().rounded(4).text_size(11.5).cursor_pointer()
      .bg(on ? th.selected : th.raised).text_color(on ? th.foreground : th.muted).hover((s) => s.text_color(th.foreground))
      .on_click((_e, cx) => action(cx)).child(label);
  }

  button(id, label, primary, enabled, th, action) {
    return div().id(id).h(30).px(14).flex().items_center().rounded(7).text_size(12.5)
      .when(primary, (el) => el.bg(enabled ? th.solid ?? th.foreground : th.raised).text_color(enabled ? th.solid_foreground ?? th.background : th.dim).font_semibold())
      .when(!primary, (el) => el.border_1().border_color(th.border))
      .when(enabled, (el) => el.cursor_pointer().hover((s) => s.opacity(0.88)).on_click((_e, cx) => action(cx)))
      .child(label);
  }

  render() {
    const th = theme(), p = this.preview;
    const root = v_flex().id("git-push").track_focus(this.handle).on_key_down((e, cx) => this.key(e, cx))
      .size_full().p(18).gap(10).bg(th.popover ?? th.raised).text_color(th.foreground).text_size(12.5)
      .child(div().text_size(14).font_semibold().child(t("title")));
    if (this.error) return root.child(div().text_color(th.ansi1).child(this.error)).child(h_flex().child(div().flex_1()).child(this.button("close", t("close"), false, true, th, () => this.close())));
    if (!p) return root.child(div().text_color(th.dim).child(t("loading")));
    // Nowhere to push: say so instead of offering an origin that isn't there.
    if (!p.remotes.length) return root.child(div().text_color(th.muted).child(t("noRemote"))).child(h_flex().child(div().flex_1()).child(this.button("close", t("close"), false, true, th, () => this.close())));
    const protectedForce = p.protected && this.force;
    const target = this.target.value().trim() || p.target;
    return root
      .child(h_flex().gap(6).items_center().flex_wrap()
        .child(div().font_semibold().child(p.branch))
        .child(div().text_color(th.dim).child("→"))
        .children(p.remotes.length > 1 ? p.remotes.map((r) => this.choice(`r-${r}`, r, r === this.remote, th, (cx) => { this.remote = r; cx.notify(); })) : [div().child(this.remote)])
        .child(div().text_color(th.dim).child("/"))
        .child(div().id("target").w(180).h(24).px(6).flex().items_center().rounded(5).border_1().border_color(th.border).bg(th.background)
          .on_mouse_down("left", (_e, cx) => cx.stop_propagation()).child(Input.new(this.target).w_full()))
        .when(p.newBranch, (el) => el.child(div().text_size(11).text_color(th.accent).child(t("new")))))
      .child(div().text_size(11.5).text_color(th.dim).child(p.commits.length === 1 ? t("commits_one") : p.commits.length ? t("commits_other", { count: p.commits.length }) : t("nothing")))
      .child(v_flex().id("commits").flex_1().min_h(0).overflow_y_scrollbar().rounded(6).border_1().border_color(th.border).bg(th.background)
        .children(p.commits.map((c, i) => h_flex().id(`c${i}`).h(24).px(8).gap(8).items_center().text_size(11.5)
          .child(div().w(64).flex_none().text_color(th.dim).child(c.short))
          .child(div().flex_1().min_w(0).truncate().child(c.subject))
          .child(div().flex_none().text_color(th.dim).child(`${c.author} · ${ago(c.time)}`))))
        .when(p.tags.length > 0, (el) => el.child(div().px(8).py(4).text_size(11).text_color(th.dim).child(t("tagsIn", { tags: p.tags.join(", ") })))))
      .child(h_flex().gap(6).items_center().flex_wrap()
        .child(div().text_size(11.5).text_color(th.muted).child(t("forceLabel")))
        .child(this.choice("f-none", t("force.none"), this.force === "", th, (cx) => { this.force = ""; this.confirmForce = false; cx.notify(); }))
        .child(this.choice("f-lease", t("force.lease"), this.force === "lease", th, (cx) => { this.force = "lease"; this.confirmForce = false; cx.notify(); }))
        .child(this.choice("f-force", t("force.force"), this.force === "force", th, (cx) => { this.force = "force"; this.confirmForce = false; cx.notify(); }))
        .child(div().w(10))
        .child(div().text_size(11.5).text_color(th.muted).child(t("tagsLabel")))
        .children(["none", "follow", "all"].map((k) => this.choice(`t-${k}`, t(`tags.${k}`), this.tags === k, th, (cx) => { this.tags = k; cx.notify(); })))
        .child(div().w(10))
        .child(this.choice("noverify", t("noVerify"), this.noVerify, th, (cx) => { this.noVerify = !this.noVerify; cx.notify(); })))
      .when(protectedForce, (el) => el.child(div().text_size(11.5).text_color(th.ansi1).child(t("protected", { target }))))
      .when(this.confirmForce && !protectedForce, (el) => el.child(div().text_size(11.5).text_color(th.blocked).child(t("confirmForce", { target }))))
      .when(Boolean(this.note), (el) => el.child(div().text_size(11.5).text_color(th.ansi1).child(this.note)))
      .child(h_flex().gap(8).child(div().flex_1())
        .child(this.button("cancel", t("cancel"), false, true, th, () => this.close()))
        .child(this.button("push", this.busy ? t("running") : this.confirmForce ? t("pushForce") : t("push"), true, !this.busy && !protectedForce && (p.commits.length > 0 || p.newBranch || Boolean(this.force) || this.tags !== "none"), th, (cx) => this.push(cx))));
  }
}
