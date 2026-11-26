import { connectionReady } from "./session-model";
import { core, type Snapshot, type NodeSummary } from "./core";
import { el, element, button, iconButton, empty } from "./dom";
import type { Modal } from "./modal";
import { readNodeFile } from "./import-file";
export function createNodes(
  modal: Modal,
  changed: () => Promise<void>,
  error: (e: unknown) => void,
) {
  let snapshot: Snapshot | null = null,
    tag = "全部",
    search = "",
    limit = 100,
    signature = "",
    busy = false;
  const list = element("node-list");
  async function action(task: () => Promise<unknown>) {
    if (busy) return;
    busy = true;
    draw(true);
    try {
      await task();
      await changed();
    } catch (e) {
      error(e);
    } finally {
      busy = false;
      draw(true);
    }
  }
  function edit(node: NodeSummary) {
    if (!snapshot) return;
    const revision = snapshot.settings.nodesRevision;
    const original = JSON.stringify([node.name, node.tags]);
    const name = el("input");
    name.value = node.name;
    name.maxLength = 128;
    const tags = el("input");
    tags.value = node.tags.join("，");
    tags.placeholder = "例如：工作，备用";
    const value = () =>
      tags.value
        .split(/[,，\n]/)
        .map((s) => s.trim())
        .filter(Boolean);
    modal.show("编辑节点", {
      dirty: () => JSON.stringify([name.value, value()]) !== original,
    });
    for (const [title, input] of [
      ["名称", name],
      ["标签", tags],
    ] as const) {
      const label = el("label", "", title);
      label.append(input);
      modal.content.append(label);
    }
    modal.content.append(
      el(
        "p",
        "hint",
        "多个标签用逗号分隔。留空时显示“默认”，修改名称和标签不会重连。",
      ),
    );
    modal.actions.append(
      button("取消", () => void modal.mayLeave()),
      button(
        "保存",
        () => {
          void modal
            .run(async () => {
              await core.profileRename(node.id, name.value, value(), revision);
              await changed();
            })
            .then((ok) => {
              if (ok) modal.close();
            });
        },
        "primary",
      ),
    );
    name.focus();
    name.select();
  }
  function importNodes() {
    if (!snapshot) return;
    const revision = snapshot.settings.nodesRevision;
    let ticket: number | null = null,
      epoch = 0,
      reading = false;
    const input = el("textarea");
    input.rows = 6;
    input.spellcheck = false;
    input.placeholder = "粘贴节点链接或 sing-box 节点 JSON，一行一个";
    input.setAttribute("aria-label", "节点链接或 JSON");
    const file = el("input");
    file.type = "file";
    file.accept = ".txt,.json,.jsonc";
    file.setAttribute("aria-label", "选择节点文件");
    const result = el("div");
    const hint = el(
      "p",
      "hint",
      "支持 SS、Trojan、VLESS、VMess、Hysteria2、TUIC。单次最多 2 MiB / 500 个节点，导入后不会自动连接。",
    );
    modal.show("导入节点", {
      dirty: () => !!input.value.trim() || reading,
      cleanup: () => {
        epoch++;
        ticket = null;
        input.value = "";
        file.value = "";
        void core.profileCancel().catch(error);
      },
    });
    const token = modal.token;
    const commit = button(
      "导入节点",
      () => {
        if (ticket === null) return;
        void modal
          .run(async () => {
            await core.profileCommit(ticket!, revision);
            await changed();
          })
          .then((ok) => {
            if (ok) modal.close();
          });
      },
      "primary",
    );
    commit.disabled = true;
    const invalidate = () => {
      epoch++;
      reading = false;
      preview.disabled = false;
      ticket = null;
      commit.disabled = true;
      result.replaceChildren();
      void core.profileCancel().catch(error);
    };
    input.oninput = invalidate;
    const preview = button("预览", () => {
      if (reading) return;
      const request = ++epoch;
      ticket = null;
      commit.disabled = true;
      preview.disabled = true;
      result.textContent = "正在检查导入内容…";
      void core
        .profilePreview(input.value)
        .then((value) => {
          if (!modal.valid(token) || request !== epoch) return;
          ticket = value.ticket;
          result.replaceChildren();
          let count = 0;
          for (const item of value.items) {
            const line = el("div", "preview-line");
            line.dataset.error = String(!!item.error);
            line.textContent = `第 ${item.line} 行 · ${item.error ?? (item.duplicate ? "重复，跳过" : (item.node?.name ?? "无法识别"))}`;
            result.append(line);
            if (item.node && !item.duplicate && !item.error) count++;
          }
          commit.textContent = `导入 ${count} 个节点`;
          commit.disabled = count === 0;
        })
        .catch((e) => {
          if (modal.valid(token) && request === epoch) {
            ticket = null;
            commit.disabled = true;
            result.replaceChildren();
            modal.error(e);
          }
        })
        .finally(() => {
          if (modal.valid(token)) preview.disabled = false;
        });
    });
    file.onchange = () => {
      invalidate();
      const selected = file.files?.[0];
      if (!selected) return;
      if (selected.size > 2 * 1024 * 1024) {
        modal.error("文件超过 2 MiB，请拆分后导入");
        file.value = "";
        return;
      }
      const request = epoch;
      reading = true;
      preview.disabled = true;
      void readNodeFile(selected)
        .then((text) => {
          if (modal.valid(token) && request === epoch) {
            input.value = text;
            modal.error(null);
          }
        })
        .catch(() => {
          if (modal.valid(token) && request === epoch)
            modal.error("文件读取失败或不是有效 UTF-8，请重新选择");
        })
        .finally(() => {
          if (modal.valid(token) && request === epoch) {
            reading = false;
            preview.disabled = false;
          }
        });
    };
    modal.content.append(input, file, hint, result);
    modal.actions.append(
      button("取消", () => void modal.mayLeave()),
      preview,
      commit,
    );
    input.focus();
  }
  function draw(force = false) {
    if (!snapshot) return;
    const s = snapshot;
    const key = JSON.stringify([
      s.settings.nodesRevision,
      s.core.runningNode?.id,
      s.core.target?.id,
      s.core.phase,
      s.runtimeBusy,
      s.settings.profile.mode,
      tag,
      search,
      limit,
      busy,
      s.core.usable,
    ]);
    if (!force && signature === key) return;
    signature = key;
    const nodes = s.settings.profile.nodes;
    const available = [
      "全部",
      ...new Set(nodes.flatMap((n) => (n.tags.length ? n.tags : ["默认"]))),
    ];
    if (!available.includes(tag)) tag = "全部";
    const chips = element("node-tags");
    chips.replaceChildren();
    for (const label of available) {
      const chip = button(label, () => {
        tag = label;
        limit = 100;
        draw(true);
      });
      chip.setAttribute("aria-pressed", String(label === tag));
      chips.append(chip);
    }
    element("nodes-count").textContent =
      `${nodes.length} 个节点 · 一次连接一个`;
    const raw = s.settings.profile.mode !== "generated";
    element("nodes-mode-hint").hidden = !raw;
    list.replaceChildren();
    const visible = nodes.filter(
      (n) =>
        (tag === "全部" || (n.tags.length ? n.tags : ["默认"]).includes(tag)) &&
        `${n.name} ${n.server} ${n.protocol} ${n.tags.join(" ")}`
          .toLowerCase()
          .includes(search),
    );
    for (const node of visible.slice(0, limit)) {
      const current = connectionReady(s) && s.core.runningNode?.id === node.id;
      const target = s.core.target?.id === node.id;
      const row = el("div", `list-row${current ? " current" : ""}`);
      row.dataset.node = node.id;
      const info = el("div", "row-info"),
        title = el("div", "row-title");
      title.append(button(node.name, () => edit(node), "link-button"));
      if (current) title.append(el("span", "tag running", "使用中"));
      if (target) title.append(el("span", "tag", "正在连接"));
      const meta = el("div", "row-meta");
      meta.append(el("span", "", `${node.server}:${node.port}`));
      for (const label of node.tags.length ? node.tags : ["默认"])
        meta.append(el("span", "tag", label));
      info.append(title, meta);
      const play = iconButton(
        current ? "stop" : "play",
        current ? `断开 ${node.name}` : `连接 ${node.name}`,
        () =>
          void action(() =>
            current ? core.action("stop") : core.connect(node.id),
          ),
        `play-button${current ? " active" : ""}`,
      );
      play.disabled = current
        ? !s.core.canStop
        : raw ||
          busy ||
          s.runtimeBusy ||
          s.closing ||
          !s.core.usable ||
          !!s.storageError;
      const remove = iconButton("remove", `删除 ${node.name}`, () => {
        modal.show("删除节点");
        modal.content.append(
          el("p", "", `删除“${node.name}”？连接凭据也会从节点库中移除。`),
        );
        modal.actions.append(
          button("取消", () => modal.close()),
          button("删除", () => {
            void modal
              .run(async () => {
                await core.profileDelete(node.id, s.settings.nodesRevision);
                await changed();
              })
              .then((ok) => {
                if (ok) modal.close();
              });
          }),
        );
      });
      remove.disabled = !!s.runtime.pid || s.runtimeBusy || busy;
      row.append(
        el(
          "span",
          "protocol-mark",
          (
            {
              trojan: "TR",
              vless: "VL",
              shadowsocks: "SS",
              vmess: "VM",
              hysteria2: "HY2",
              tuic: "TU",
            } as Record<string, string>
          )[node.protocol] ?? node.protocol.slice(0, 3).toUpperCase(),
        ),
        info,
        play,
        remove,
      );
      list.append(row);
    }
    if (!visible.length)
      list.append(
        empty(
          nodes.length ? "没有匹配节点" : "还没有节点",
          nodes.length
            ? "换个标签或搜索词。"
            : "导入分享链接或本地文件，预览后添加。",
        ),
      );
    element("more-nodes").hidden = visible.length <= limit;
    element<HTMLButtonElement>("import-nodes").disabled =
      !!s.storageError || s.closing;
  }
  element("import-nodes").onclick = importNodes;
  element<HTMLInputElement>("node-search").oninput = (e) => {
    search = (e.target as HTMLInputElement).value.trim().toLowerCase();
    limit = 100;
    draw();
  };
  element("more-nodes").onclick = () => {
    limit += 100;
    draw();
  };
  return {
    render(s: Snapshot) {
      snapshot = s;
      draw();
    },
  };
}
