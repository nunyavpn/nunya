/**
 * A very small DOM helper.
 *
 * Deliberately not a framework. The app has four screens and its most dynamic content is a list
 * that changes when a subscription refreshes — that does not justify a runtime dependency in a VPN
 * client, where every shipped byte is something a user has to trust.
 */

type Attrs = Record<string, string | number | boolean | EventListener | undefined | null>;
type Child = Node | string | number | null | undefined | false;

/** Creates an element. Keys starting with `on` become listeners; everything else is an attribute. */
export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs?: Attrs,
  ...children: Child[]
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);

  for (const [key, value] of Object.entries(attrs ?? {})) {
    if (value === undefined || value === null || value === false) continue;

    if (key.startsWith("on") && typeof value === "function") {
      node.addEventListener(key.slice(2).toLowerCase(), value as EventListener);
    } else if (key === "class") {
      node.className = String(value);
    } else if (key === "html") {
      // Only ever used with strings this app composed itself, never with server or user content.
      node.innerHTML = String(value);
    } else if (value === true) {
      node.setAttribute(key, "");
    } else {
      node.setAttribute(key, String(value));
    }
  }

  append(node, children);
  return node;
}

/** Creates an SVG element. SVG needs its own namespace or the browser renders nothing. */
export function svg(tag: string, attrs?: Record<string, string | number>, ...children: Node[]) {
  const node = document.createElementNS("http://www.w3.org/2000/svg", tag);
  for (const [key, value] of Object.entries(attrs ?? {})) {
    node.setAttribute(key, String(value));
  }
  for (const child of children) node.appendChild(child);
  return node;
}

export function append(parent: Node, children: Child[]) {
  for (const child of children) {
    if (child === null || child === undefined || child === false) continue;
    parent.appendChild(typeof child === "object" ? child : document.createTextNode(String(child)));
  }
}

/** Replaces a node's contents in one pass, so a re-render never shows a half-empty list. */
export function render(parent: Element, ...children: Child[]) {
  const frag = document.createDocumentFragment();
  append(frag, children);
  parent.replaceChildren(frag);
}

export function qs<T extends Element>(selector: string, root: ParentNode = document): T {
  const node = root.querySelector<T>(selector);
  if (!node) throw new Error(`missing element: ${selector}`);
  return node;
}
