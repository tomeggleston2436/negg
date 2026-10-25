// SPDX-License-Identifier: Apache-2.0
// Tab order inside a modal: only real tab stops. In a radio group the
// browser tabs to the checked radio only (or the first enabled one when none
// is checked), so the other radios are not stops.

export interface FocusCandidate {
  tag: string;
  type: string;
  name: string;
  checked: boolean;
  disabled: boolean;
  tabIndex: number;
}

/** Indices of `items` (in document order) that are tab stops. */
export function tabStops(items: readonly FocusCandidate[]): number[] {
  const checkedGroups = new Set(
    items.filter((it) => isRadio(it) && it.checked && !it.disabled).map((it) => it.name),
  );
  const seenGroups = new Set<string>();
  const out: number[] = [];
  items.forEach((it, i) => {
    if (it.disabled || it.tabIndex < 0) return;
    if (isRadio(it) && it.name !== "") {
      if (checkedGroups.has(it.name) ? !it.checked : seenGroups.has(it.name)) return;
      seenGroups.add(it.name);
    }
    out.push(i);
  });
  return out;
}

function isRadio(it: FocusCandidate): boolean {
  return it.tag === "input" && it.type === "radio";
}

/** Reads the candidate facts from DOM elements. */
export function describe(el: HTMLElement): FocusCandidate {
  const input = el instanceof HTMLInputElement ? el : null;
  return {
    tag: el.tagName.toLowerCase(),
    type: input?.type ?? "",
    name: input?.name ?? "",
    checked: input?.checked ?? false,
    disabled: el.hasAttribute("disabled"),
    tabIndex: el.tabIndex,
  };
}

/** True for an element that takes typed text (its own keys, its own undo). */
export function isTextEntry(t: EventTarget | null): boolean {
  if (typeof HTMLElement === "undefined" || !(t instanceof HTMLElement)) return false;
  if (t.isContentEditable || t instanceof HTMLTextAreaElement) return true;
  return t instanceof HTMLInputElement && (t.type === "text" || t.type === "number" || t.type === "search");
}
