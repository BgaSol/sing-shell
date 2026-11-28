import type { RouteAction } from "./api";
import { el } from "./dom";

export function exitControl(
  value: RouteAction,
  name: string,
  label: string,
  change: (exit: RouteAction) => void,
) {
  const group = el("div", "routing-exit");
  group.setAttribute("role", "radiogroup");
  group.setAttribute("aria-label", label);
  for (const [exit, text] of [
    ["proxy", "代理"],
    ["direct", "直连"],
  ] as const) {
    const option = el("label"),
      input = el("input");
    input.type = "radio";
    input.name = name;
    input.value = exit;
    input.checked = value === exit;
    input.setAttribute("aria-label", `${label}：${text}`);
    input.onchange = () => {
      if (input.checked) change(exit);
    };
    option.append(input, el("span", "", text));
    group.append(option);
  }
  return group;
}

export function routingSwitch(
  text: string,
  label: string,
  change: (checked: boolean) => void,
) {
  const wrapper = el("label", "routing-switch"),
    input = el("input"),
    caption = el("span", "", text);
  input.type = "checkbox";
  input.setAttribute("aria-label", label);
  input.onchange = () => change(input.checked);
  const control = el("span", "switch");
  control.append(input, el("span"));
  wrapper.append(caption, control);
  return { wrapper, input, caption };
}
