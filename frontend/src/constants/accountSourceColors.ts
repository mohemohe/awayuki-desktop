import type { AccountSourceColor } from "../types/app";
import type { MessageId } from "../i18n";

export const ACCOUNT_SOURCE_COLORS: Array<{
  value: AccountSourceColor;
  label: MessageId;
  hex: string;
}> = [
  { value: "Transparent", label: "Transparent", hex: "transparent" },
  { value: "Mauve", label: "Mauve", hex: "#cba6f7" },
  { value: "Red", label: "Red", hex: "#f38ba8" },
  { value: "Peach", label: "Peach", hex: "#fab387" },
  { value: "Yellow", label: "Yellow", hex: "#f9e2af" },
  { value: "Green", label: "Green", hex: "#a6e3a1" },
  { value: "Sapphire", label: "Sapphire", hex: "#74c7ec" },
  { value: "Lavender", label: "Lavender", hex: "#b4befe" },
];

export function accountSourceColorHex(color?: AccountSourceColor | null) {
  if (!color || color === "Transparent") return undefined;
  return ACCOUNT_SOURCE_COLORS.find((item) => item.value === color)?.hex;
}
