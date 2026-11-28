import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";

const moduleUrl = (source) =>
  `data:text/javascript;base64,${Buffer.from(stripTypeScriptTypes(source)).toString("base64")}`;
const domUrl = moduleUrl(
  readFileSync(new URL("../src/dom.ts", import.meta.url), "utf8"),
);
const { Modal } = await import(
  moduleUrl(
    readFileSync(new URL("../src/modal.ts", import.meta.url), "utf8").replace(
      '"./dom"',
      JSON.stringify(domUrl),
    ),
  )
);

// Only the DOM operations needed for editor ownership and focus are simulated.
class Element {
  childNodes = [];
  isConnected = true;
  visible = true;
  open = false;
  onkeydown = null;
  parent = null;
  append(...children) {
    for (const child of children) {
      if (child.parent)
        child.parent.childNodes = child.parent.childNodes.filter(
          (c) => c !== child,
        );
      child.parent = this;
      this.childNodes.push(child);
    }
  }
  replaceChildren(...children) {
    for (const child of this.childNodes) child.parent = null;
    this.childNodes = [];
    this.append(...children);
  }
  focus() {
    document.activeElement = this;
  }
  showModal() {
    this.open = true;
  }
  close() {
    this.open = false;
  }
  getClientRects() {
    return this.visible ? [{}] : [];
  }
}
function setup() {
  const elements = new Map();
  const header = new Element(),
    nav = new Element(),
    trigger = new Element();
  globalThis.document = {
    activeElement: trigger,
    createElement: () => new Element(),
    getElementById(id) {
      if (!elements.has(id)) elements.set(id, new Element());
      return elements.get(id);
    },
    querySelector: (selector) => (selector.startsWith("main") ? header : nav),
  };
  return { modal: new Modal(), trigger, header };
}
test("unsaved decisions suspend editor shortcuts and restore them when continuing", async () => {
  const { modal } = setup();
  modal.show("Editor", { dirty: () => true });
  const field = new Element();
  modal.content.append(field);
  field.focus();
  let shortcuts = 0;
  const keydown = () => shortcuts++;
  modal.content.onkeydown = keydown;
  const leaving = modal.mayLeave();
  assert.equal(modal.content.onkeydown, null);
  modal.content.onkeydown?.({ key: "Enter", ctrlKey: true });
  assert.equal(shortcuts, 0);
  const confirm = modal.content.childNodes[1];
  const actions = confirm.childNodes[2];
  actions.childNodes[1].onclick(); // Continue editing.
  assert.equal(await leaving, false);
  assert.equal(modal.content.onkeydown, keydown);
  assert.deepEqual(modal.content.childNodes, [field]);
  assert.equal(document.activeElement, field);
});
test("closing returns focus to a visible action when the original row was removed or hidden", () => {
  for (const state of ["removed", "hidden", "visible"]) {
    const { modal, trigger, header } = setup();
    modal.show("Editor");
    trigger.isConnected = state !== "removed";
    trigger.visible = state !== "hidden";
    modal.close();
    assert.equal(
      document.activeElement,
      state === "visible" ? trigger : header,
    );
  }
});
