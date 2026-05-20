import { Globe, Lock, LockOpen, Mail } from "lucide-react";
import { t } from "../../i18n";

export function VisibilityIcon({ visibility }: { visibility: string }) {
  const className = "h-3 w-3";
  if (visibility === "unlisted")
    return <LockOpen className={className} aria-label={t("Unlisted")} />;
  if (visibility === "private")
    return <Lock className={className} aria-label={t("Private")} />;
  if (visibility === "direct")
    return <Mail className={className} aria-label={t("Direct")} />;
  return <Globe className={className} aria-label={t("Public")} />;
}
