export function element<T extends HTMLElement = HTMLElement>(id: string): T {
  const result = document.getElementById(id);
  if (!result) throw new Error(`Missing element ${id}`);
  return result as T;
}
export function errorText(reason: unknown): string {
  return typeof reason === "object" && reason !== null && "message" in reason
    ? String(reason.message)
    : String(reason);
}
export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className = "",
  text?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}
export function button(text: string, onclick: () => void, className = "") {
  const b = el("button", className, text);
  b.type = "button";
  b.onclick = onclick;
  return b;
}
export function icon(
  name: "play" | "stop" | "edit" | "remove" | "up" | "down" | "grip",
) {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("class", "icon");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path.setAttribute(
    "d",
    {
      play: "m8 5 11 7-11 7z",
      stop: "M6 6h12v12H6z",
      edit: "m14 4 6 6M4 20l5-1L20 8a2 2 0 0 0-4-4L5 15z",
      remove: "M5 7h14M9 7V4h6v3M7 7l1 13h8l1-13M10 11v5m4-5v5",
      up: "m6 15 6-6 6 6",
      down: "m6 9 6 6 6-6",
      grip: "M9 5h.1M15 5h.1M9 12h.1M15 12h.1M9 19h.1M15 19h.1",
    }[name],
  );
  svg.append(path);
  return svg;
}
export function iconButton(
  name: Parameters<typeof icon>[0],
  label: string,
  onclick: () => void,
  className = "icon-button",
) {
  const b = button("", onclick, className);
  b.setAttribute("aria-label", label);
  b.title = label;
  b.append(icon(name));
  return b;
}
export function empty(title: string, description: string) {
  const row = el("div", "empty");
  row.append(el("strong", "", title), el("p", "", description));
  return row;
}
export function bytes(value: number | null) {
  if (value === null) return "采样中";
  if (value < 1024) return `${value} B`;
  if (value < 1048576) return `${(value / 1024).toFixed(1)} KiB`;
  if (value < 1073741824) return `${(value / 1048576).toFixed(1)} MiB`;
  return `${(value / 1073741824).toFixed(2)} GiB`;
}
