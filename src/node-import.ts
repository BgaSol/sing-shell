import { core } from "./api";
import { el, button } from "./dom";
import { readNodeFile } from "./import-file";
import type { Modal } from "./modal";

let cleanup: Promise<unknown> = Promise.resolve();
export async function importNodes(
  modal: Modal,
  revision: number,
  changed: () => Promise<void>,
) {
  await cleanup;
  if (modal.open) return;
  let epoch = 0,
    ticket: number | null = null,
    work: Promise<void> | null = null,
    timer: number | undefined,
    fileReading = false;
  let closed = false,
    parsed = -1;
  let previewRevision = revision;
  const input = el("textarea"),
    file = el("input"),
    result = el("div");
  input.rows = 6;
  input.spellcheck = false;
  input.placeholder = "粘贴节点链接或 sing-box 节点 JSON，一行一个";
  input.setAttribute("aria-label", "节点链接或 JSON");
  file.type = "file";
  file.accept = ".txt,.json,.jsonc";
  file.setAttribute("aria-label", "选择节点文件");
  modal.show("导入节点", {
    dirty: () => !!input.value.trim() || fileReading,
    cleanup: () => {
      closed = true;
      epoch++;
      ticket = null;
      window.clearTimeout(timer);
      input.value = "";
      file.value = "";
      cleanup = (async () => {
        await core.profileCancel();
        if (work) await work;
        await core.profileCancel();
      })().catch(() => {});
    },
  });
  const token = modal.token,
    commit = button("导入节点", () => void save(), "primary");
  commit.disabled = true;
  function invalidate() {
    epoch++;
    ticket = null;
    fileReading = false;
    commit.disabled = true;
    modal.error(null);
    result.textContent = input.value.trim() ? "正在解析…" : "";
    window.clearTimeout(timer);
  }
  function schedule() {
    invalidate();
    timer = window.setTimeout(() => void preview(), 300);
  }
  async function preview(): Promise<void> {
    if (
      work ||
      closed ||
      fileReading ||
      !input.value.trim() ||
      parsed === epoch
    )
      return;
    const requested = epoch,
      text = input.value;
    parsed = requested;
    work = (async () => {
      try {
        await core.profileCancel();
        if (closed || requested !== epoch) return;
        const value = await core.profilePreview(text);
        if (closed || requested !== epoch || !modal.valid(token)) return;
        ticket = value.ticket;
        previewRevision = value.nodesRevision;
        result.replaceChildren();
        const count = value.items.filter(
          (i) => i.node && !i.error && !i.duplicate,
        ).length;
        const duplicates = value.items.filter((i) => i.duplicate).length,
          failures = value.items.filter((i) => i.error).length;
        result.append(
          el(
            "p",
            "hint",
            `${count} 个有效 · ${duplicates} 个重复 · ${failures} 个错误`,
          ),
        );
        const details = el("details");
        details.open = failures > 0;
        details.append(el("summary", "", "查看导入明细"));
        for (const item of value.items) {
          const row = el(
            "div",
            "preview-line",
            `第 ${item.line} 行 · ${item.error ?? (item.duplicate ? "重复，跳过" : (item.node?.name ?? "无法识别"))}`,
          );
          row.dataset.error = String(!!item.error);
          details.append(row);
        }
        result.append(details);
        commit.textContent = `导入 ${count} 个有效节点`;
        commit.disabled = count === 0;
      } catch (e) {
        if (!closed && requested === epoch) {
          ticket = null;
          result.replaceChildren();
          modal.error(e);
        }
      } finally {
        work = null;
        if (!closed && requested !== epoch) void preview();
      }
    })();
    await work;
  }
  async function save() {
    if (ticket === null || commit.disabled || fileReading) {
      modal.error("请等待最新内容解析完成");
      return false;
    }
    const selected = ticket;
    const ok = await modal.run(async () => {
      await core.profileCommit(selected, previewRevision);
      await changed();
    });
    if (ok) modal.close();
    else {
      ticket = null;
      commit.disabled = true;
      parsed = -1;
      void preview();
    }
    return ok;
  }
  input.oninput = schedule;
  file.onchange = () => {
    invalidate();
    const selected = file.files?.[0];
    if (!selected) return;
    const requested = epoch;
    fileReading = true;
    void readNodeFile(selected)
      .then((text) => {
        if (!closed && requested === epoch) {
          input.value = text;
          fileReading = false;
          schedule();
        }
      })
      .catch((e) => {
        if (!closed && requested === epoch) modal.error(e);
      })
      .finally(() => {
        if (requested === epoch) fileReading = false;
      });
  };
  modal.onSubmit(save, "导入并继续");
  modal.content.append(
    input,
    file,
    el(
      "p",
      "hint",
      "支持 SS、Trojan、VLESS、VMess、Hysteria2、TUIC。单次最多 2 MiB / 500 个节点，导入后不会自动连接。",
    ),
    result,
  );
  modal.actions.append(
    button("取消", () => void modal.mayLeave()),
    commit,
  );
  input.focus();
}
