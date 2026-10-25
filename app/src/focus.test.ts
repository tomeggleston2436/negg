// SPDX-License-Identifier: Apache-2.0
import { describe as group, expect, it } from "vitest";
import { type FocusCandidate, tabStops } from "./focus";

const radio = (name: string, checked: boolean, disabled = false): FocusCandidate => ({
  tag: "input",
  type: "radio",
  name,
  checked,
  disabled,
  tabIndex: 0,
});
const button = (disabled = false): FocusCandidate => ({
  tag: "button",
  type: "",
  name: "",
  checked: false,
  disabled,
  tabIndex: 0,
});

group("tabStops", () => {
  it("keeps only the checked radio of each group", () => {
    const items = [radio("scope", false), radio("scope", true), radio("scope", false), radio("format", true), radio("format", false), button()];
    expect(tabStops(items)).toEqual([1, 3, 5]);
  });

  it("uses the first enabled radio when none is checked, and skips disabled or untabbable items", () => {
    const items = [radio("scope", false, true), radio("scope", false), radio("scope", false), button(true), { ...button(), tabIndex: -1 }, button()];
    expect(tabStops(items)).toEqual([1, 5]);
  });

  it("ignores a checked radio that is disabled", () => {
    expect(tabStops([radio("s", true, true), radio("s", false)])).toEqual([1]);
  });
});
