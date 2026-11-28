import { core, type Network } from "./api";
import { el, button, iconButton } from "./dom";
import type { Modal } from "./modal";
export async function editDownloadNetwork(
  modal: Modal,
  current: Network,
  refresh: () => Promise<void>,
) {
  const mirrors = await core.mirrors(),
    draft = structuredClone(current),
    original = JSON.stringify(current);
  modal.show("下载网络", { dirty: () => JSON.stringify(draft) !== original });
  const label = el("label", "source-choice"),
    fallback = el("input");
  fallback.type = "checkbox";
  fallback.checked = draft.fallbackEnabled;
  fallback.onchange = () => (draft.fallbackEnabled = fallback.checked);
  label.append(fallback, document.createTextNode("原始地址失败时尝试备用线路"));
  modal.content.append(label);
  const builtins = el("details");
  builtins.append(el("summary", "", "内置备用线路"));
  for (const mirror of mirrors) {
    const row = el("label", "source-choice"),
      box = el("input");
    box.type = "checkbox";
    box.checked = !draft.disabledBuiltins.includes(mirror.id);
    box.onchange = () => {
      draft.disabledBuiltins = draft.disabledBuiltins.filter(
        (id) => id !== mirror.id,
      );
      if (!box.checked) draft.disabledBuiltins.push(mirror.id);
    };
    row.append(box, document.createTextNode(mirror.prefix));
    builtins.append(row);
  }
  const list = el("div");
  function rows() {
    list.replaceChildren();
    draft.customMirrors.forEach((mirror, index) => {
      const row = el("div", "mirror-row"),
        box = el("input"),
        name = el("input"),
        prefix = el("input");
      box.type = "checkbox";
      box.checked = mirror.enabled;
      box.setAttribute("aria-label", `启用地址 ${index + 1}`);
      box.onchange = () => (mirror.enabled = box.checked);
      name.value = mirror.name;
      name.placeholder = "名称";
      name.setAttribute("aria-label", `地址 ${index + 1} 名称`);
      name.oninput = () => (mirror.name = name.value);
      prefix.type = "url";
      prefix.value = mirror.prefix;
      prefix.placeholder = "https://";
      prefix.setAttribute("aria-label", `地址 ${index + 1} HTTPS 前缀`);
      prefix.oninput = () => (mirror.prefix = prefix.value);
      const up = iconButton("up", "上移地址", () => {
        [draft.customMirrors[index - 1], draft.customMirrors[index]] = [
          draft.customMirrors[index],
          draft.customMirrors[index - 1],
        ];
        rows();
      });
      up.disabled = index === 0;
      row.append(
        box,
        name,
        prefix,
        up,
        iconButton("remove", "移除地址", () => {
          draft.customMirrors.splice(index, 1);
          rows();
        }),
      );
      list.append(row);
    });
  }
  modal.content.append(
    builtins,
    el("h3", "", "自定义 HTTPS 前缀"),
    list,
    button("添加地址", () => {
      if (draft.customMirrors.length >= 10) {
        modal.error("最多 10 个地址");
        return;
      }
      draft.customMirrors.push({ name: "", prefix: "", enabled: true });
      rows();
    }),
  );
  rows();
  const diagnose = el("details"),
    input = el("input"),
    output = el("pre");
  input.type = "url";
  input.placeholder = "留空测试原始地址";
  input.setAttribute("aria-label", "诊断 HTTPS 前缀");
  const diagnosticLabel = el("label", "", "测试地址");
  diagnosticLabel.append(input);
  diagnose.append(
    el("summary", "", "线路诊断"),
    diagnosticLabel,
    button("测试地址", () => {
      void modal.run(async () => {
        const results = await core.diagnose(input.value.trim() || null);
        output.textContent = results
          .map(
            (r) => `${r.name}：${r.ok ? "通过" : r.error}（${r.elapsedMs} ms）`,
          )
          .join("\n");
      });
    }),
    output,
  );
  modal.content.append(
    diagnose,
    el("p", "hint", "保存仅用于新任务，正在下载的任务继续使用原线路。"),
  );
  const save = async () => {
    const ok = await modal.run(async () => {
      await core.saveNetwork(current.revision, draft);
      await refresh();
    });
    if (ok) modal.close();
    return ok;
  };
  modal.onSubmit(save);
  modal.actions.append(
    button("取消", () => void modal.mayLeave()),
    button("保存", () => void save(), "primary"),
  );
}
