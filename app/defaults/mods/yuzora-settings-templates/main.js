// Settings > Yuzora > Templates: herdr-core's agent templates on each
// machine (`hc templates`, <config>/herdr-core/templates/<name>.md). Add and
// edit them (the header as a form, the body in an editor, checked before
// saving), move them to the trash, start an agent from one. Owned by lane L6.
//
// Reading and writing go through yuzora:settings (Rust), which reads the
// header as hc does and reaches other machines with `hc exec --on`.
// Starting one opens yuzora-settings-launch (space, tab, task).
//
// Props (all optional): { machine, edit } — `edit`: a template's name opens
// its form at once ("" a new one), e.g. tab.new {mod: "yuzora-settings-templates", props: {edit: "reviewer"}}.

import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, EditorState, Editor } from "gpui-base";
import { theme, machines, knownAgents, openOverlay, props } from "herdr";
import { t } from "yuzora:core";
import { listTemplates, checkTemplate, templateText, saveTemplate, trashTemplate, reveal } from "yuzora:settings";
import { page, title, heading, button, segmented, note, badge, spinner, field, input } from "./ui.js";

const MONO = "JetBrains Mono";
const BLANK = { command: "", description: "", ready: "", submit: "", stop: "", paste_gap_ms: "" };
const INPUTS = ["name", "command", "description", "submit", "stop", "paste_gap_ms"];

export default class Templates extends View {
  init(_props, cx) {
    const p = props() ?? {};
    this.machine = typeof p.machine === "string" && p.machine ? p.machine : null; // null: this computer
    this.pendingEdit = typeof p.edit === "string" ? p.edit : null;
    this.list = null;
    this.loading = false;
    this.editing = null; // { original, expected, others, ready }
    this.issues = [];
    this.focused = null;
    this.confirm = null; // a template name waiting for "move to trash"
    this.message = null; // { text, tone }
    this.agents = [];
    this.inputs = {};
    for (const key of INPUTS) {
      const s = InputState.new({ placeholder: t(`field.${key}_placeholder`) });
      s.on("change", (_e, cx) => this.recheck(cx));
      s.on("focus", (_e, cx) => { this.focused = key; cx.notify(); });
      s.on("blur", (_e, cx) => { if (this.focused === key) this.focused = null; cx.notify(); });
      this.inputs[key] = s;
    }
    this.body = EditorState.new({ language: "markdown", soft_wrap: true, line_numbers: false, folding: false, font_size: 12.5 });
    this.body.on("change", (_e, cx) => this.recheck(cx));
    this.reload(cx);
  }

  reload(cx) {
    this.loading = true;
    const machine = this.machine;
    cx.spawn(async (cx) => {
      const list = await listTemplates(machine);
      if (machine === this.machine) {
        this.list = list;
        this.loading = false;
        if (this.pendingEdit !== null) {
          this.edit(list.templates.find((e) => e.name === this.pendingEdit) ?? null, cx);
          this.pendingEdit = null;
        }
      }
      cx.notify();
    });
  }

  pickMachine(name, cx) {
    if (name === this.machine) return;
    this.machine = name;
    this.list = null;
    this.editing = null;
    this.confirm = null;
    this.message = null;
    this.reload(cx);
    cx.notify();
  }

  // Editing --------------------------------------------------------------

  fields() {
    const f = { ...BLANK };
    for (const key of Object.keys(BLANK)) if (this.inputs[key]) f[key] = this.inputs[key].value();
    f.ready = this.editing?.ready ?? "";
    return f;
  }

  recheck(cx) {
    if (!this.editing) return;
    this.issues = checkTemplate(this.inputs.name.value().trim(), this.fields(), this.body.value());
    cx.notify();
  }

  edit(entry, cx) {
    const f = entry ? entry.fields : BLANK;
    this.editing = { original: entry?.name ?? null, expected: entry ? entry.text : null, others: entry?.others ?? [], ready: f.ready === "quiet" ? "quiet" : f.ready, preview: false };
    this.inputs.name.set_value(entry?.name ?? "");
    for (const key of Object.keys(BLANK)) if (this.inputs[key]) this.inputs[key].set_value(f[key] ?? "");
    this.body.set_value(entry?.body ?? "");
    this.message = null;
    this.confirm = null;
    this.recheck(cx);
    if (!entry) this.inputs.name.focus();
    const machine = this.machine;
    cx.spawn(async (cx) => {
      try {
        this.agents = (await knownAgents({ on: machine })).filter((a) => a.installed);
      } catch {
        this.agents = [];
      }
      cx.notify();
    });
    cx.notify();
  }

  save(cx) {
    const e = this.editing;
    if (!e || this.saving || this.issues.some((i) => i.level === "error")) return;
    this.saving = true;
    const name = this.inputs.name.value().trim();
    const machine = this.machine;
    cx.spawn(async (cx) => {
      const r = await saveTemplate(machine, name, this.fields(), e.others, this.body.value(), e.expected, e.original);
      this.saving = false;
      if (r.ok || r.code === "old_kept") {
        // old_kept: renamed, but the old file is still there beside the new one.
        this.editing = null;
        this.message = r.ok ? { text: t("saved", { name }), tone: "ok" } : { text: t("errors.old_kept", { name, old: e.original }), tone: "warning" };
        this.reload(cx);
      } else {
        this.message = { text: this.why(r, name), tone: "error", changed: r.code === "changed" };
      }
      cx.notify();
    });
    cx.notify();
  }

  why(r, name) {
    const known = ["exists", "changed", "invalid", "name_invalid", "missing"];
    if (known.includes(r.code)) return t(`errors.${r.code}`, { name });
    return t("errors.other", { message: r.message ?? r.code ?? "" });
  }

  trash(name, cx) {
    const machine = this.machine;
    this.confirm = null;
    cx.spawn(async (cx) => {
      const r = await trashTemplate(machine, name);
      this.message = r.ok ? { text: t("trashed", { name }), tone: "ok" } : { text: this.why(r, name), tone: "error" };
      this.reload(cx);
      cx.notify();
    });
  }

  launch(entry) {
    openOverlay({ mod: "yuzora-settings-launch", kind: "dialog", width: 560, props: { template: entry.name, machine: this.machine, closeAlso: props()?.overlay ?? null } });
  }

  // Drawing --------------------------------------------------------------

  machineBar(th, cx) {
    const names = [null, ...machines().map((m) => m.name).filter((n) => n)];
    return h_flex()
      .gap(8)
      .items_center()
      .flex_wrap()
      .child(div().text_size(12).text_color(th.dim).child(t("machine")))
      .child(segmented(th, "tp-m", names.map((n) => [n ?? "", n ?? t("local")]), this.machine ?? "", (v, cx) => this.pickMachine(v || null, cx)))
      .child(div().flex_1())
      .when(this.loading, (el) => el.child(spinner(th)))
      .child(button(th, "tp-reload", t("reload"), (_e, cx) => this.reload(cx)))
      .child(button(th, "tp-new", t("new"), (_e, cx) => this.edit(null, cx), "primary"));
  }

  errorText(e) {
    const base = t(`hc.${e.code}`);
    return e.detail ? `${base} ${e.detail}` : base;
  }

  entryRow(th, e) {
    const asking = this.confirm === e.name;
    const broken = Boolean(e.error);
    const controls = asking
      ? h_flex()
          .gap(6)
          .child(button(th, `tp-no-${e.name}`, t("cancel"), (_e, cx) => { this.confirm = null; cx.notify(); }))
          .child(button(th, `tp-yes-${e.name}`, t("trash"), (_e, cx) => this.trash(e.name, cx), "danger"))
      : h_flex()
          .gap(6)
          .child(button(th, `tp-run-${e.name}`, t("start"), () => this.launch(e), "primary", broken))
          .child(button(th, `tp-edit-${e.name}`, t("edit"), (_e, cx) => this.edit(e, cx)))
          .child(button(th, `tp-del-${e.name}`, t("trash"), (_e, cx) => { this.confirm = e.name; cx.notify(); }));
    return v_flex()
      .w_full()
      .py(10)
      .gap(4)
      .border_b(1)
      .border_color(th.border)
      .child(
        h_flex()
          .gap(10)
          .items_center()
          .child(
            v_flex()
              .flex_1()
              .min_w(0)
              .gap(3)
              .child(
                h_flex()
                  .gap(8)
                  .items_center()
                  .child(div().text_size(13.5).font_semibold().text_color(th.foreground).child(e.name))
                  .when(broken, (el) => el.child(badge(th, t("broken"), th.ansi1))),
              )
              .when(Boolean(e.fields.description), (el) => el.child(div().text_size(12).text_color(th.muted).child(e.fields.description)))
              .when(Boolean(e.fields.command), (el) => el.child(div().font_family(MONO).text_size(11.5).text_color(th.dim).child(e.fields.command))),
          )
          .child(controls.flex_none()),
      )
      .when(asking, (el) => el.child(note(th, t("trash_ask", { name: e.name }), "warning")))
      .when(broken, (el) => el.child(note(th, this.errorText(e.error), "error")));
  }

  listView(th, cx) {
    const l = this.list;
    const body = v_flex().w_full();
    if (!l) return body.child(h_flex().py(14).gap(8).items_center().child(spinner(th)).child(note(th, t("reading"))));
    if (l.error) body.child(v_flex().py(10).gap(4).child(note(th, t("read_failed"), "error")).child(note(th, l.error)));
    if (l.dir)
      body.child(
        h_flex()
          .py(6)
          .gap(8)
          .items_center()
          .child(div().flex_1().min_w(0).font_family(MONO).text_size(11.5).text_color(th.dim).child(l.dir))
          .when(!this.machine, (el) => el.child(button(th, "tp-folder", t("open_folder"), (_e, cx) => { this.openFolder(l.dir); cx.notify(); }))),
      );
    if (!l.error && !l.templates.length) body.child(v_flex().py(14).gap(4).child(note(th, t("empty"))).child(note(th, t("empty_about"))));
    return body.children(l.templates.map((e) => this.entryRow(th, e)));
  }

  openFolder(dir) {
    try {
      reveal(dir);
    } catch {
      // The folder appears with the first template.
      this.message = { text: t("no_folder"), tone: "muted" };
    }
  }

  issuesFor(key) {
    return this.issues.filter((i) => i.field === key);
  }

  hint(key) {
    const own = this.issuesFor(key);
    if (own.length) {
      const i = own.find((x) => x.level === "error") ?? own[0];
      return [t(`issues.${i.code}`), i.level === "error" ? "error" : "warning"];
    }
    return [t(`field.${key}_hint`), null];
  }

  inputField(th, key, mono) {
    const [hint, tone] = this.hint(key);
    return field(th, t(`field.${key}`), input(th, this.inputs[key], this.focused === key, mono ? MONO : null), hint, tone);
  }

  // Installed agents as commands to start from: a click fills the command.
  agentChips(th) {
    if (!this.agents.length) return null;
    const quote = (a) => (/^[\w@%+=:,./-]+$/.test(a) ? a : `'${a.replace(/'/g, "'\\''")}'`);
    return h_flex()
      .gap(6)
      .flex_wrap()
      .items_center()
      .child(div().text_size(11.5).text_color(th.dim).child(t("agents")))
      .children(
        this.agents.slice(0, 8).map((a) =>
          div()
            .id(`tp-agent-${a.key}`)
            .px(8)
            .py(2)
            .rounded(6)
            .border_1()
            .border_color(th.border)
            .text_size(11.5)
            .text_color(th.muted)
            .cursor_pointer()
            .hover((s) => s.bg(th.hover).text_color(th.foreground))
            .on_click((_e, cx) => {
              this.inputs.command.set_value(a.run.map(quote).join(" "));
              this.recheck(cx);
            })
            .child(a.name),
        ),
      );
  }

  editView(th, cx) {
    const e = this.editing;
    const blocked = this.issues.some((i) => i.level === "error");
    const bodyHint = this.issuesFor("body")[0];
    const text = e.preview ? templateText(this.fields(), e.others, this.body.value()) : null;
    return v_flex()
      .w_full()
      .gap(12)
      .child(
        h_flex()
          .items_center()
          .gap(8)
          .child(div().flex_1().text_size(14).font_semibold().text_color(th.foreground).child(e.original ? t("editing", { name: e.original }) : t("adding")))
          .child(note(th, this.machine ? t("on_machine", { machine: this.machine }) : t("on_local"))),
      )
      .child(h_flex().gap(12).child(div().flex_1().child(this.inputField(th, "name"))).child(div().flex_1().child(this.inputField(th, "description"))))
      .child(this.inputField(th, "command", true))
      .child(this.agentChips(th) ?? div())
      .child(
        field(
          th,
          t("field.ready"),
          h_flex().child(
            segmented(th, "tp-ready", [["", t("ready.status")], ["quiet", t("ready.quiet")]], e.ready === "quiet" ? "quiet" : "", (v, cx) => {
              e.ready = v;
              this.recheck(cx);
            }),
          ),
          this.issuesFor("ready").length ? t("issues.ready_unknown") : t(e.ready === "quiet" ? "ready.quiet_about" : "ready.status_about"),
          this.issuesFor("ready").length ? "warning" : null,
        ),
      )
      .when(e.ready === "quiet" || Boolean(this.inputs.submit.value() || this.inputs.stop.value() || this.inputs.paste_gap_ms.value()), (el) =>
        el.child(
          h_flex()
            .gap(12)
            .child(div().flex_1().child(this.inputField(th, "submit", true)))
            .child(div().flex_1().child(this.inputField(th, "stop", true)))
            .child(div().flex_1().child(this.inputField(th, "paste_gap_ms", true))),
        ),
      )
      .when(Boolean(e.others.length), (el) => el.child(note(th, t("others", { keys: e.others.map(([k]) => k).join(", ") }))))
      .child(
        field(
          th,
          t("field.body"),
          Editor.new(this.body).h(200).w_full().rounded(7).border_1().border_color(th.input).bg(th.background),
          bodyHint ? t(`issues.${bodyHint.code}`) : t("field.body_hint"),
          bodyHint ? "warning" : null,
        ),
      )
      .when(Boolean(text), (el) =>
        el.child(
          v_flex()
            .p(10)
            .rounded(7)
            .border_1()
            .border_color(th.border)
            .bg(th.background)
            .children(text.split("\n").map((line) => div().font_family(MONO).text_size(11.5).line_height("16px").text_color(th.muted).child(line || " "))),
        ),
      )
      .child(
        h_flex()
          .gap(8)
          .items_center()
          .child(button(th, "tp-preview", e.preview ? t("hide_file") : t("show_file"), (_e, cx) => { e.preview = !e.preview; cx.notify(); }))
          .child(div().flex_1())
          .when(Boolean(this.saving), (el) => el.child(spinner(th)))
          .child(button(th, "tp-cancel", t("cancel"), (_e, cx) => { this.editing = null; this.message = null; cx.notify(); }))
          .child(button(th, "tp-save", t("save"), (_e, cx) => this.save(cx), "primary", blocked || Boolean(this.saving))),
      );
  }

  render(cx) {
    const th = theme();
    const m = this.message;
    return page(
      th,
      "tp-body",
      v_flex()
        .w_full()
        .gap(6)
        .child(title(th, t("page.title"), t("page.about")))
        .child(this.machineBar(th, cx))
        .when(Boolean(m), (el) =>
          el.child(
            h_flex()
              .gap(8)
              .items_center()
              .child(note(th, m.text, m.tone))
              .when(Boolean(m.changed), (el) =>
                el.child(button(th, "tp-reread", t("reread"), (_e, cx) => {
                  this.editing = null;
                  this.message = null;
                  this.reload(cx);
                })),
              ),
          ),
        )
        .child(heading(th, this.editing ? t("form") : t("list")))
        .child(this.editing ? this.editView(th, cx) : this.listView(th, cx)),
    );
  }
}
