import React from "react";
import { Activity, Clock3, Database, SendHorizontal } from "lucide-react";
import { useAppStore } from "../../store/appStore";
import { formatNumber, formatUptime } from "../../utils/format";
import { t } from "../../i18n";

export function StatusBar() {
  const snapshot = useAppStore((state) => state.snapshot);
  const statusBar = useAppStore((state) => state.statusBar);
  const statusMessage = useAppStore((state) => state.statusMessage);
  const loadStatusBar = useAppStore((state) => state.loadStatusBar);
  const outboxItems = useAppStore((state) => state.composeOutboxItems);
  const [now, setNow] = React.useState(() => Date.now());

  React.useEffect(() => {
    void loadStatusBar();
    const metricsTimer = window.setInterval(() => void loadStatusBar(), 15_000);
    const clockTimer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => {
      window.clearInterval(metricsTimer);
      window.clearInterval(clockTimer);
    };
  }, [loadStatusBar]);

  const statusCount =
    statusBar?.statusCount ?? snapshot?.database.statusCount ?? 0;
  const recentStatusCount = statusBar?.recentStatusCount ?? 0;
  const uptimeSeconds = statusBar
    ? statusBar.uptimeSeconds + Math.floor((now - statusBar.fetchedAt) / 1000)
    : 0;
  const activeOutboxItems = outboxItems.filter(
    (item) => !["succeeded", "cancelled"].includes(item.state),
  );
  const outboxNeedsAttention = activeOutboxItems.some((item) =>
    ["failed", "uncertain"].includes(item.state),
  );

  return (
    <footer className="flex h-5 shrink-0 items-center justify-between gap-3 border-t border-surface0 bg-base-300 px-1.5 text-[11px] text-overlay0">
      <div
        className="min-w-0 truncate"
        role="status"
        aria-live="polite"
        aria-atomic="true"
      >
        {statusMessage}
      </div>
      <div className="flex shrink-0 items-center gap-2">
        <button
          type="button"
          className={`inline-flex items-center gap-0.5 tabular-nums ${
            outboxNeedsAttention
              ? "text-red"
              : activeOutboxItems.length > 0
                ? "text-yellow"
                : ""
          }`}
          title={t("Send queue")}
          aria-label={`${t("Send queue")}: ${activeOutboxItems.length}`}
          onClick={() => useAppStore.setState({ composeOutboxOpen: true })}
        >
          <SendHorizontal className="h-3 w-3" />
          {formatNumber(activeOutboxItems.length)}
        </button>
        <StatusBarMetric
          icon={<Database className="h-3 w-3" />}
          value={formatNumber(statusCount)}
          title={t("SQLite statuses")}
        />
        <StatusBarMetric
          icon={<Activity className="h-3 w-3" />}
          value={formatNumber(recentStatusCount)}
          title={t("Statuses created in the last 15 minutes")}
        />
        <StatusBarMetric
          icon={<Clock3 className="h-3 w-3" />}
          value={formatUptime(uptimeSeconds)}
          title={t("Application uptime")}
        />
      </div>
    </footer>
  );
}

function StatusBarMetric({
  icon,
  value,
  title,
}: {
  icon: React.ReactNode;
  value: string;
  title: string;
}) {
  return (
    <span
      className="inline-flex items-center gap-0.5 tabular-nums"
      title={title}
    >
      {icon}
      {value}
    </span>
  );
}
