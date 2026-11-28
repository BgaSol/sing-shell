import { element, el, button, errorText } from "./dom";
export class Modal {
  readonly dialog = element<HTMLDialogElement>("workspace-modal");
  readonly content = element("modal-content");
  readonly actions = element("modal-actions");
  private dirty = () => false;
  private cleanup = () => {};
  private submit: (() => Promise<boolean>) | null = null;
  private submitLabel = "保存并继续";
  private work: Promise<boolean> | null = null;
  private generation = 0;
  private trigger: HTMLElement | null = null;
  private deciding: Promise<boolean> | null = null;
  constructor() {
    element("modal-close").onclick = () => void this.mayLeave();
    this.dialog.oncancel = (e) => {
      e.preventDefault();
      void this.mayLeave();
    };
  }
  get pending() {
    return this.open && this.dirty();
  }
  get open() {
    return this.dialog.open;
  }
  get token() {
    return this.generation;
  }
  valid(token: number) {
    return this.open && token === this.generation;
  }
  show(
    title: string,
    options: { dirty?: () => boolean; cleanup?: () => void } = {},
  ) {
    if (this.open) throw Error("请先完成当前编辑");
    this.trigger = document.activeElement as HTMLElement;
    this.generation++;
    this.dirty = options.dirty ?? (() => false);
    this.cleanup = options.cleanup ?? (() => {});
    this.submit = null;
    this.content.replaceChildren();
    this.actions.replaceChildren();
    this.actions.hidden = false;
    this.content.onkeydown = null;
    this.error(null);
    element("modal-title").textContent = title;
    this.dialog.showModal();
  }
  onSubmit(submit: () => Promise<boolean>, label = "保存并继续") {
    this.submit = submit;
    this.submitLabel = label;
  }
  close() {
    if (this.work) return;
    this.generation++;
    this.cleanup();
    this.submit = null;
    this.content.replaceChildren();
    this.actions.replaceChildren();
    this.dialog.close();
    (this.trigger?.isConnected
      ? this.trigger
      : element("import-nodes")
    ).focus();
  }
  error(reason: unknown) {
    const e = element("modal-error");
    e.textContent = reason == null ? "" : errorText(reason);
    e.hidden = reason == null;
  }
  run(task: () => Promise<void>): Promise<boolean> {
    if (this.work) return this.work;
    this.content.inert = true;
    this.dialog.setAttribute("aria-busy", "true");
    const controls = [
      ...this.actions.querySelectorAll<HTMLButtonElement>("button"),
      element<HTMLButtonElement>("modal-close"),
    ];
    const disabled = controls.map((b) => b.disabled);
    controls.forEach((b) => (b.disabled = true));
    this.work = (async () => {
      try {
        await Promise.resolve().then(task);
        this.error(null);
        return true;
      } catch (e) {
        this.error(e);
        return false;
      } finally {
        this.content.inert = false;
        this.dialog.setAttribute("aria-busy", "false");
        controls.forEach((b, i) => (b.disabled = disabled[i]));
        this.work = null;
      }
    })();
    return this.work;
  }
  async mayLeave(exiting = false): Promise<boolean> {
    if (this.work) await this.work;
    if (!this.open) return true;
    if (!this.dirty()) {
      this.close();
      return true;
    }
    if (this.deciding) return this.deciding;
    const focus = document.activeElement as HTMLElement;
    const wrapper = el("div", "editor-held");
    wrapper.append(...this.content.childNodes);
    wrapper.inert = true;
    const confirm = el("div", "modal-confirm");
    confirm.append(
      el("strong", "", "本次编辑尚未保存"),
      el("p", "hint", "保留输入继续编辑，或放弃本次修改。"),
    );
    this.content.append(wrapper, confirm);
    this.actions.hidden = true;
    this.deciding = new Promise((resolve) => {
      const restore = () => {
        wrapper.inert = false;
        this.content.replaceChildren(...wrapper.childNodes);
        this.actions.hidden = false;
        this.deciding = null;
      };
      const back = button("继续编辑", () => {
        restore();
        focus?.focus();
        resolve(false);
      });
      const discard = button("放弃并继续", () => {
        restore();
        this.close();
        resolve(true);
      });
      const actions = el("div", "actions");
      if (exiting && this.submit)
        actions.append(
          button(
            this.submitLabel,
            () => {
              const submit = this.submit!;
              restore();
              void submit().then(resolve);
            },
            "primary",
          ),
        );
      actions.append(discard, back);
      confirm.append(actions);
      back.focus();
    });
    return this.deciding;
  }
  async decision(
    title: string,
    message: string,
  ): Promise<"save" | "discard" | "cancel"> {
    this.show(title);
    this.content.append(el("p", "", message));
    return new Promise((resolve) => {
      let value: "save" | "discard" | "cancel" = "cancel";
      for (const [label, result] of [
        ["确认并继续", "save"],
        ["取消", "cancel"],
      ] as const)
        this.actions.append(
          button(
            label,
            () => {
              value = result;
              this.close();
            },
            result === "save" ? "primary" : "",
          ),
        );
      this.cleanup = () => resolve(value);
    });
  }
}
