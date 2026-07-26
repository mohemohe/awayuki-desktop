import type { AccountSourceColor } from "../types/app";
import type { MessageId } from "../i18n";

export const ACCOUNT_SOURCE_COLORS: Array<{
  value: AccountSourceColor;
  label: MessageId;
  cssColor: string;
}> = [
  { value: "Transparent", label: "Transparent", cssColor: "transparent" },
  { value: "Mauve", label: "Mauve", cssColor: "rgb(var(--ctp-mauve))" },
  { value: "Red", label: "Red", cssColor: "rgb(var(--ctp-red))" },
  { value: "Peach", label: "Peach", cssColor: "rgb(var(--ctp-peach))" },
  { value: "Yellow", label: "Yellow", cssColor: "rgb(var(--ctp-yellow))" },
  { value: "Green", label: "Green", cssColor: "rgb(var(--ctp-green))" },
  {
    value: "Sapphire",
    label: "Sapphire",
    cssColor: "rgb(var(--ctp-sapphire))",
  },
  {
    value: "Lavender",
    label: "Lavender",
    cssColor: "rgb(var(--ctp-lavender))",
  },
];

export function accountSourceCssColor(color?: AccountSourceColor | null) {
  if (!color || color === "Transparent") return undefined;
  return ACCOUNT_SOURCE_COLORS.find((item) => item.value === color)?.cssColor;
}
