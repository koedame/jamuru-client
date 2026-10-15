/**
 * What both ways of pointing at the screen need to know about its elements: a
 * path that finds one again, and whether it is on screen. The element picker for
 * an LLM (`elementReport.ts`, development and beta builds) and the marks that
 * show where the other person of a settings help is looking (`presence.ts`)
 * share these, so they agree on what an element is.
 */

/** Ids a framework makes up per render (React's `useId` is `:r1:`); they differ from one window to the next. */
function isMadeUp(id: string): boolean {
  return !/^[A-Za-z][A-Za-z0-9_-]*$/.test(id) || /^(radix|react)-/i.test(id);
}

/**
 * A selector that finds this element again, ending at the nearest id.
 *
 * With `stable`, an id a framework made up does not count: the path must read
 * the same in another window that draws the same screen.
 */
export function cssPath(el: Element, options: { stable?: boolean } = {}): string {
  const parts: string[] = [];
  let node: Element | null = el;
  while (node && parts.length < 8) {
    let part = node.nodeName.toLowerCase();
    if (node.id && !(options.stable && isMadeUp(node.id))) {
      parts.unshift(`${part}#${node.id}`);
      break;
    }
    const testId = node.getAttribute("data-testid");
    if (testId) {
      parts.unshift(`${part}[data-testid="${testId}"]`);
      break;
    }
    const parent: Element | null = node.parentElement;
    if (parent) {
      const same = [...parent.children].filter((c) => c.nodeName === node!.nodeName);
      if (same.length > 1) part += `:nth-of-type(${same.indexOf(node) + 1})`;
    }
    parts.unshift(part);
    node = parent;
  }
  return parts.join(" > ");
}

/** Whether the element takes up room on the screen and is not hidden. */
export function isVisible(el: Element): boolean {
  const rect = el.getBoundingClientRect();
  if (rect.width === 0 || rect.height === 0) return false;
  const style = getComputedStyle(el);
  return style.display !== "none" && style.visibility !== "hidden" && style.opacity !== "0";
}
