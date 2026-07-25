import { hasTauriRuntime, invokeTypedCommand } from "../api/tauri";

export function getClientPlatform(): "macos" | "windows" | "linux" | "unknown" {
  const text = `${navigator.userAgent} ${navigator.platform}`.toLowerCase();
  if (text.includes("mac")) return "macos";
  if (text.includes("win")) return "windows";
  if (text.includes("linux") || text.includes("x11")) return "linux";
  return "unknown";
}

export async function copyToClipboard(text: string) {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }

  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.setAttribute("readonly", "true");
  textarea.style.position = "fixed";
  textarea.style.opacity = "0";
  document.body.appendChild(textarea);
  textarea.select();
  document.execCommand("copy");
  textarea.remove();
}

export async function openExternalUrl(url: string) {
  const parsed = new URL(url);
  if (parsed.protocol !== "https:" && parsed.protocol !== "http:") {
    throw new Error("Unsupported URL scheme");
  }
  if (hasTauriRuntime()) {
    await invokeTypedCommand("open_status_url", { url: parsed.toString() });
    return;
  }
  window.open(parsed.toString(), "_blank", "noopener,noreferrer");
}
