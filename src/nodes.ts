import { connectionReady } from "./session-model";
import { core, type Snapshot, type NodeSummary } from "./api";
import { el, element, button, empty } from "./dom";
import { importNodes } from "./node-import";
import type { Modal } from "./modal";
export function createNodes(
  modal: Modal,
  changed: () => Promise<void>,
  error: (e: unknown) => void,
  beforeNetwork: () => Promise<void>,
) {
  let snapshot: Snapshot | null = null,
    tag = "全部",
    search = "",
    limit = 100,
    signature = "",
    busy = false;
  async function connect(id: string) {
    if (busy) return;
    busy = true;
    draw();
    try {
      await beforeNetwork();
      await core.connect(id);
      await changed();
    } catch (e) {
      error(e);
    } finally {
      busy = false;
      draw();
    }
  }
  function edit(node: NodeSummary) {
    if (!snapshot) return;
    const revision = snapshot.settings.nodesRevision,
      name = el("input"),
      tags = el("input");
    name.value = node.name;
    name.maxLength = 128;
    tags.value = node.tags.join("，");
    tags.placeholder = "工作，备用";
    const value = () =>
        tags.value
          .split(/[,，\n]/)
          .map((s) => s.trim())
          .filter(Boolean),
      original = JSON.stringify([name.value, value()]);
    modal.show("编辑节点", {
      dirty: () => original !== JSON.stringify([name.value, value()]),
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
        `${node.protocol.toUpperCase()} · ${node.server}:${node.port}`,
      ),
    );
    const save = async () => {
      const ok = await modal.run(async () => {
        await core.profileRename(node.id, name.value, value(), revision);
        await changed();
      });
      if (ok) modal.close();
      return ok;
    };
    modal.onSubmit(save);
    let confirming = false;
    const remove = button(
      "删除节点",
      () => {
        if (!confirming) {
          confirming = true;
          remove.textContent = "确认删除";
          modal.content.append(
            el("p", "hint", "确认后会从本机节点库移除此节点和凭据。"),
          );
          return;
        }
        void modal
          .run(async () => {
            await core.profileDelete(node.id, revision);
            await changed();
          })
          .then((ok) => {
            if (ok) modal.close();
          });
      },
      "quiet danger",
    );
    remove.disabled =
      !!snapshot.runtime.pid ||
      snapshot.runtimeBusy ||
      snapshot.runtime.cleanupPending;
    if (remove.disabled) remove.title = "断开并完成清理后可删除";
    modal.actions.append(
      remove,
      button("取消", () => void modal.mayLeave()),
      button("保存", () => void save(), "primary"),
    );
    name.focus();
    name.select();
  }
  function draw() {
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
      s.core.canConnect,
      s.storageError,
      s.closing,
    ]);
    if (signature === key) return;
    signature = key;
    const nodes = s.settings.profile.nodes,
      labels = [...new Set(nodes.flatMap((n) => n.tags))];
    if (tag !== "全部" && !labels.includes(tag)) tag = "全部";
    const chips = element("node-tags");
    chips.replaceChildren();
    if (labels.length)
      for (const label of ["全部", ...labels]) {
        const chip = button(label, () => {
          tag = label;
          limit = 100;
          draw();
        });
        chip.setAttribute("aria-pressed", String(tag === label));
        chips.append(chip);
      }
    element("nodes-count").textContent = `${nodes.length} 个`;
    element("nodes-mode-hint").hidden = s.settings.profile.mode === "generated";
    const list = element("node-list");
    list.replaceChildren();
    const visible = nodes.filter(
      (n) =>
        (tag === "全部" || n.tags.includes(tag)) &&
        `${n.name} ${n.server} ${n.protocol} ${n.tags.join(" ")}`
          .toLowerCase()
          .includes(search),
    );
    for (const node of visible.slice(0, limit)) {
      const current = connectionReady(s) && s.core.runningNode?.id === node.id,
        target = s.core.target?.id === node.id;
      const row = el("div", `list-row${current ? " current" : ""}`);
      row.dataset.node = node.id;
      const info = el("div", "row-info"),
        title = el("div", "row-title");
      title.append(el("strong", "", node.name));
      if (current) title.append(el("span", "tag running", "使用中"));
      else if (target) title.append(el("span", "tag", "正在连接"));
      else if (s.settings.profile.selectedNode === node.id)
        title.append(el("span", "tag", "下次连接"));
      const meta = el("div", "row-meta");
      meta.append(el("span", "", `${node.server}:${node.port}`));
      node.tags.forEach((t) => meta.append(el("span", "tag", t)));
      info.append(title, meta);
      const editButton = button("编辑", () => edit(node), "quiet");
      editButton.setAttribute("aria-label", `编辑 ${node.name}`);
      const play = button(
        s.runtime.pid ? "切换" : "连接",
        () => void connect(node.id),
        "play-button",
      );
      play.setAttribute(
        "aria-label",
        `${s.runtime.pid ? "切换到" : "连接"} ${node.name}`,
      );
      play.disabled = busy || !s.core.canConnect;
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
        editButton,
      );
      if (!current) row.append(play);
      list.append(row);
    }
    if (!visible.length)
      list.append(
        empty(
          nodes.length ? "没有匹配节点" : "还没有节点",
          nodes.length
            ? "换个标签或搜索词。"
            : "导入分享链接或本地文件，即可开始连接。",
        ),
      );
    element("more-nodes").hidden = visible.length <= limit;
    element<HTMLButtonElement>("import-nodes").disabled =
      !!s.storageError || s.closing;
  }
  element("import-nodes").onclick = () => {
    if (snapshot)
      void importNodes(modal, snapshot.settings.nodesRevision, changed).catch(
        error,
      );
  };
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
