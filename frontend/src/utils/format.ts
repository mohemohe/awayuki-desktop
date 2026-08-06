import type { AppearanceSettings, TimelineStatus } from "../types/app";
import { intlFormatter } from "../i18n";

export function filenameFromPath(path: string) {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? "media";
}

export function filenameFromUrl(value: string) {
  try {
    const url = new URL(value);
    return filenameFromPath(decodeURIComponent(url.pathname));
  } catch {
    return filenameFromPath(value);
  }
}

export function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}

export function computeMediaFitScale(width: number, height: number) {
  if (!width || !height) return 1;
  const availableWidth = Math.max(1, window.innerWidth - 96);
  const availableHeight = Math.max(1, window.innerHeight - 128);
  return Math.min(1, availableWidth / width, availableHeight / height);
}

export function formatUptime(totalSeconds: number) {
  const seconds = Math.max(0, Math.floor(totalSeconds));
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const rest = seconds % 60;
  return `${hours}:${String(minutes).padStart(2, "0")}:${String(rest).padStart(2, "0")}`;
}

export function avatarShapeClass(shape: AppearanceSettings["avatar_shape"]) {
  if (shape === "Circle") return "rounded-full";
  if (shape === "Square") return "rounded-none";
  return "rounded";
}

export function formatDuration(totalSeconds: number) {
  const seconds = Math.max(0, Math.floor(totalSeconds));
  const minutes = Math.floor(seconds / 60);
  if (minutes < 1) return `${seconds}s`;
  return `${minutes}m ${seconds % 60}s`;
}

export function statusUrl(status: TimelineStatus) {
  if (status.url) return status.url;
  return `https://${status.serverDomain}/@${status.acct.replace(/^@/, "")}/${status.originalStatusId}`;
}

export function formatCompactNumber(value: number) {
  return intlFormatter(
    (locale) =>
      new Intl.NumberFormat(locale, {
        notation: "compact",
        maximumFractionDigits: 1,
      }),
  ).format(value);
}

export function formatNumber(value: number) {
  return intlFormatter((locale) => new Intl.NumberFormat(locale)).format(value);
}

export function statusPlainText(status: TimelineStatus) {
  const text = htmlToPlainText(status.content);
  return [status.spoilerText, text].filter(Boolean).join("\n").trim();
}

export function htmlToPlainText(html: string) {
  if (typeof document === "undefined") {
    return html
      .replace(/<br\s*\/?\s*>/gi, "\n")
      .replace(
        /<\/(?:address|article|aside|blockquote|div|footer|h[1-6]|header|li|main|nav|ol|p|pre|section|table|tr|ul)\s*>/gi,
        "\n",
      )
      .replace(/<[^>]+>/g, "")
      .trim();
  }
  const element = document.createElement("div");
  element.innerHTML = html;
  decodeNestedHtmlTextEntities(element);
  return plainTextFromNode(element).trim();
}

const blockElements = new Set([
  "ADDRESS",
  "ARTICLE",
  "ASIDE",
  "BLOCKQUOTE",
  "DIV",
  "FOOTER",
  "H1",
  "H2",
  "H3",
  "H4",
  "H5",
  "H6",
  "HEADER",
  "LI",
  "MAIN",
  "NAV",
  "OL",
  "P",
  "PRE",
  "SECTION",
  "TABLE",
  "TR",
  "UL",
]);

function plainTextFromNode(root: ParentNode) {
  let text = "";

  const append = (node: Node) => {
    if (node.nodeType === Node.TEXT_NODE) {
      text += node.nodeValue ?? "";
      return;
    }
    if (!(node instanceof Element)) return;
    if (node.tagName === "BR") {
      text += "\n";
      return;
    }

    node.childNodes.forEach(append);
    if (blockElements.has(node.tagName) && !text.endsWith("\n")) {
      text += "\n";
    }
  };

  root.childNodes.forEach(append);
  return text;
}

export function decodeNestedHtmlTextEntities(root: ParentNode) {
  if (typeof document === "undefined") return;

  const decoder = document.createElement("textarea");
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  let node = walker.nextNode();
  while (node) {
    const textNode = node as Text;
    textNode.nodeValue = decodeNestedHtmlEntities(
      textNode.nodeValue ?? "",
      decoder,
    );
    node = walker.nextNode();
  }
}

function decodeNestedHtmlEntities(value: string, decoder: HTMLTextAreaElement) {
  let decoded = value;
  for (let depth = 0; depth < 8; depth += 1) {
    const next = decoded.replace(
      /&(?:#\d+|#x[\da-f]+|[a-z][\da-z]+);/gi,
      (entity) => {
        decoder.innerHTML = entity;
        return decoder.value;
      },
    );
    if (next === decoded) break;
    decoded = next;
  }
  return decoded;
}

export function formatTime(value: string) {
  const date = new Date(value);
  if (Number.isNaN(date.valueOf())) return value;
  const now = new Date();
  const time = [date.getHours(), date.getMinutes(), date.getSeconds()]
    .map((item) => String(item).padStart(2, "0"))
    .join(":");
  if (
    date.getFullYear() === now.getFullYear() &&
    date.getMonth() === now.getMonth() &&
    date.getDate() === now.getDate()
  ) {
    return time;
  }
  const day = [
    date.getFullYear(),
    String(date.getMonth() + 1).padStart(2, "0"),
    String(date.getDate()).padStart(2, "0"),
  ].join("/");
  return `${day} ${time}`;
}

export function escapeHtml(value: string) {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;");
}
