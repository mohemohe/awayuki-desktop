/** Small deterministic PR-gate audit for the semantics covered by our primitives. */
export function basicAccessibilityViolations(
  root: ParentNode = document,
): string[] {
  const violations: string[] = [];
  for (const dialog of root.querySelectorAll<HTMLElement>('[role="dialog"]')) {
    if (!dialog.getAttribute("aria-label") && !dialog.getAttribute("aria-labelledby")) {
      violations.push("dialog has no accessible name");
    }
    if (dialog.getAttribute("aria-modal") !== "true") {
      violations.push("dialog is not modal");
    }
  }
  for (const tab of root.querySelectorAll<HTMLElement>('[role="tab"]')) {
    if (!tab.hasAttribute("aria-selected")) {
      violations.push("tab has no selected state");
    }
  }
  for (const option of root.querySelectorAll<HTMLElement>('[role="option"]')) {
    if (!option.hasAttribute("aria-selected")) {
      violations.push("listbox option has no selected state");
    }
  }
  for (const menuItem of root.querySelectorAll<HTMLElement>('[role="menuitem"]')) {
    if (!(menuItem.textContent?.trim() || menuItem.getAttribute("aria-label"))) {
      violations.push("menu item has no accessible name");
    }
  }
  return violations;
}

