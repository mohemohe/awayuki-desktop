import React from "react";
import { t } from "../../i18n";
import { useAppStore } from "../../store/appStore";
import type { AppearanceSettings } from "../../types/app";
import { openExternalUrl } from "../../utils/browser";

export function statusFontSizeClass(fontSize: AppearanceSettings["font_size"]) {
  if (fontSize === "Small") return "status-size-small";
  if (fontSize === "Large") return "status-size-large";
  return "status-size-medium";
}

export function statusVisibilityBackgroundClass(
  enabled: boolean,
  visibility: string,
) {
  if (!enabled) return "";
  if (visibility.toLowerCase() === "unlisted") {
    return "status-visibility-unlisted";
  }
  if (["private", "direct"].includes(visibility.toLowerCase())) {
    return "status-visibility-private";
  }
  return "";
}

export function statusHoverBackgroundClass(
  visibilityBackgroundClass: string,
) {
  return visibilityBackgroundClass ? "" : "hover:bg-surface0/40";
}

export function statusItemStyle(
  paddingLeft: number | undefined,
  borderLeftColor: string | undefined,
) {
  if (paddingLeft === undefined && !borderLeftColor) return undefined;
  const style: React.CSSProperties = {};
  if (paddingLeft !== undefined) style.paddingLeft = paddingLeft;
  if (borderLeftColor) style.borderLeftColor = borderLeftColor;
  return style;
}

export function QuoteLinkPreview({ url }: { url: string }) {
  return (
    <button
      className="mt-2 block max-w-full overflow-hidden rounded border border-surface1 bg-base-300/50 p-2 text-left text-xs text-blue hover:border-blue/60"
      onClick={() =>
        void openExternalUrl(url).catch((error) =>
          useAppStore.setState({ error: String(error) }),
        )
      }
      title={t("Open quoted post")}
    >
      {url}
    </button>
  );
}
