import { element, el, button } from "./dom";
export class Modal {
  readonly dialog = element<HTMLDialogElement>("workspace-modal");
  readonly content = element("modal-content");
  readonly actions = element("modal-actions");
  private dirty = () => false;
  private cleanup = () => {};
  private busy = false;
  private generation = 0;
  private trigger: HTMLElement | null = null;
  private originRow: { type: "node" | "policy"; id: string } | null = null;
  private deciding: Promise<boolean> | null = null;
  constructor() {
    element("modal-close").onclick = () => void this.requestClose();
    this.dialog.oncancel = (e) => {
      e.preventDefault();
      void this.requestClose();
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
    if (this.open) throw new Error("请先完成当前编辑");
    this.trigger = document.activeElement as HTMLElement;
    const row = this.trigger?.closest<HTMLElement>(
      "[data-node], [data-policy]",
    );
    this.originRow = row
      ? {
          type: row.dataset.node ? "node" : "policy",
          id: row.dataset.node ?? row.dataset.policy!,
        }
      : null;
    this.generation++;
    this.dirty = options.dirty ?? (() => false);
    this.cleanup = options.cleanup ?? (() => {});
    this.content.replaceChildren();
    this.content.onkeydown = null;
    this.actions.replaceChildren();
    this.actions.hidden = false;
    this.error(null);
    element("modal-title").textContent = title;
    this.dialog.showModal();
  }
  close() {
    if (this.busy) return;
    this.generation++;
    this.cleanup();
    this.content.replaceChildren();
    this.actions.replaceChildren();
    this.dialog.close();
    if (this.trigger?.isConnected) this.trigger.focus();
    else {
      const origin = this.originRow;
      const row = origin
        ? Array.from(
            document.querySelectorAll<HTMLElement>(`[data-${origin.type}]`),
          ).find(
            (row) =>
              (origin.type === "node"
                ? row.dataset.node
                : row.dataset.policy) === origin.id,
          )
        : null;
      (
        row?.querySelector<HTMLButtonElement>("button") ??
        element<HTMLButtonElement>("import-nodes")
      ).focus();
    }
  }
  error(reason: unknown) {
    const error = element("modal-error");
    error.textContent = reason === null ? "" : String(reason);
    error.hidden = reason === null;
  }
  setBusy(value: boolean) {
    this.busy = value;
    this.content.inert = value;
    this.dialog.setAttribute("aria-busy", String(value));
    element<HTMLButtonElement>("modal-close").disabled = value;
    for (const b of this.actions.querySelectorAll<HTMLButtonElement>("button"))
      b.disabled = value;
  }
  async run(task: () => Promise<void>) {
    if (this.busy) return false;
    this.setBusy(true);
    try {
      await task();
      this.error(null);
      return true;
    } catch (e) {
      this.error(e);
      return false;
    } finally {
      this.setBusy(false);
    }
  }
  private async requestClose() {
    if (this.busy) return;
    if (!this.dirty()) {
      this.close();
      return;
    }
    await this.mayLeave();
  }
  async mayLeave(): Promise<boolean> {
    while (this.busy)
      await new Promise((resolve) => window.setTimeout(resolve, 25));
    if (!this.open) return true;
    if (!this.dirty()) {
      this.close();
      return true;
    }
    if (this.deciding) return this.deciding;
    const editingFocus = document.activeElement as HTMLElement;
    const wrapper = el("div");
    wrapper.style.display = "contents";
    wrapper.append(...Array.from(this.content.childNodes));
    wrapper.inert = true;
    const confirm = el("div", "modal-confirm");
    confirm.append(
      el("strong", "", "本次编辑尚未完成"),
      el("p", "", "放弃后会清除当前弹窗中的输入。"),
    );
    this.content.append(wrapper, confirm);
    this.actions.hidden = true;
    this.deciding = new Promise((resolve) => {
      const back = button("继续编辑", () => {
        wrapper.inert = false;
        confirm.remove();
        this.content.replaceChildren(...Array.from(wrapper.childNodes));
        this.actions.hidden = false;
        this.deciding = null;
        editingFocus?.focus();
        resolve(false);
      });
      const discard = button("放弃并继续", () => {
        this.actions.hidden = false;
        this.deciding = null;
        this.close();
        resolve(true);
      });
      const actions = el("div", "actions");
      actions.append(back, discard);
      confirm.append(actions);
      back.focus();
    });
    return this.deciding;
  }
  async decision(
    title: string,
    message: string,
  ): Promise<"save" | "discard" | "cancel"> {
    if (this.open) throw new Error("请先完成当前弹窗");
    this.show(title);
    this.content.append(el("p", "", message));
    return new Promise((resolve) => {
      let outcome: "save" | "discard" | "cancel" = "cancel";
      for (const [label, value] of [
        ["保存并继续", "save"],
        ["放弃并继续", "discard"],
        ["返回编辑", "cancel"],
      ] as const) {
        this.actions.append(
          button(label, () => {
            outcome = value;
            this.close();
          }),
        );
      }
      this.cleanup = () => resolve(outcome);
    });
  }
}
