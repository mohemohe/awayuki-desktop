import React from "react";
import { RefreshCw, X, Zap } from "lucide-react";
import { invokeTypedCommand, invokeTypedReadCommand } from "../../api/tauri";
import { getAppLocale, t } from "../../i18n";
import { useAppStore } from "../../store/appStore";
import type { WebSocketStatus } from "../../types/app";
import { formatNumber } from "../../utils/format";
import { Dialog } from "../primitives/Dialog";

export function WebSocketStatusControl() {
  const open = useAppStore((state) => state.webSocketStatusOpen);
  const [statuses, setStatuses] = React.useState<WebSocketStatus[]>([]);
  const [loaded, setLoaded] = React.useState(false);
  const [loadError, setLoadError] = React.useState(false);
  const [reconnectError, setReconnectError] = React.useState(false);
  const [pending, setPending] = React.useState(false);
  const pendingRef = React.useRef(false);
  const generation = React.useRef(0);

  React.useEffect(() => {
    let active = true;
    let inFlight = false;
    const refresh = async () => {
      if (inFlight || pendingRef.current) return;
      inFlight = true;
      const requestGeneration = generation.current;
      try {
        const next = await invokeTypedReadCommand("get_web_socket_statuses");
        if (active && generation.current === requestGeneration) {
          setStatuses(next);
          setLoaded(true);
          setLoadError(false);
        }
      } catch {
        if (active && generation.current === requestGeneration) setLoadError(true);
      } finally {
        inFlight = false;
      }
    };
    void refresh();
    const timer = window.setInterval(() => void refresh(), 1_000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, []);

  const reconnect = async (id: string | null) => {
    if (pendingRef.current) return;
    pendingRef.current = true;
    generation.current += 1;
    setPending(true);
    setReconnectError(false);
    try {
      await invokeTypedCommand("reconnect_web_socket", { id });
    } catch {
      setReconnectError(true);
    } finally {
      pendingRef.current = false;
      setPending(false);
    }
  };
  const close = () => useAppStore.setState({ webSocketStatusOpen: false });
  const count = statuses.filter((status) => status.state === "connected").length;

  return (
    <>
      <button
        type="button"
        className={`inline-flex items-center gap-0.5 tabular-nums ${loadError ? "text-yellow" : ""}`}
        title={loadError ? t("Unable to load WebSocket status.") : t("WebSocket status")}
        aria-label={`${t("WebSocket status")}: ${loadError || !loaded ? "—" : count}`}
        onClick={() => useAppStore.setState({ webSocketStatusOpen: true })}
      >
        <Zap className="h-3 w-3" />
        {loadError || !loaded ? "—" : formatNumber(count)}
      </button>
      <Dialog open={open} onClose={close} labelledBy="websocket-status-title" className="modal modal-open">
        <section className="modal-box flex max-h-[80vh] max-w-4xl flex-col rounded-md border border-surface0 bg-base-100 p-0">
          <header className="flex shrink-0 items-center gap-3 border-b border-surface0 px-4 py-3">
            <Zap className="h-4 w-4 text-blue" />
            <h2 id="websocket-status-title" className="min-w-0 flex-1 text-base font-semibold text-text">{t("WebSocket status")}</h2>
            <button
              type="button"
              className="btn btn-circle btn-ghost btn-xs"
              disabled={pending || statuses.length === 0}
              onClick={() => void reconnect(null)}
              title={t("Reconnect all")}
              aria-label={t("Reconnect all")}
            >
              <RefreshCw className="h-3.5 w-3.5" />
            </button>
            <button
              type="button"
              className="btn btn-circle btn-ghost btn-xs"
              onClick={close}
              title={t("Close")}
              aria-label={t("Close")}
            >
              <X className="h-3.5 w-3.5" />
            </button>
          </header>
          <div className="min-h-0 flex-1 overflow-y-auto">
            {loadError || reconnectError ? <p role="alert" className="px-4 py-3 text-sm text-red">{reconnectError ? t("Unable to reconnect WebSocket.") : t("Unable to load WebSocket status.")}</p> : null}
            {statuses.length === 0 ? <p className="grid min-h-40 place-items-center px-6 text-sm text-overlay0">{loaded ? t("No WebSockets.") : t("Loading...")}</p> : (
              <ul className="divide-y divide-surface0">
                {statuses.map((status) => (
                  <li key={status.id} className="flex items-start gap-3 px-4 py-3">
                    <dl className="grid min-w-0 flex-1 grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-xs">
                      <dt className="text-overlay0">{t("Account")}</dt><dd className="break-all text-text">{status.account}</dd>
                      <dt className="text-overlay0">{t("Server")}</dt><dd className="break-all text-subtext1">{status.server}</dd>
                      <dt className="text-overlay0">{t("Stream type")}</dt><dd className="break-words text-subtext1">{status.streamType}</dd>
                      <dt className="text-overlay0">{t("Status")}</dt><dd className={status.state === "connected" ? "text-green" : "text-yellow"}>{stateLabel(status.state)}</dd>
                      <dt className="text-overlay0">{t("Last ping sent")}</dt><dd className="text-subtext1">{timestamp(status.lastPingAt)}</dd>
                      <dt className="text-overlay0">{t("Last pong received")}</dt><dd className="text-subtext1">{timestamp(status.lastPongAt)}</dd>
                      <dt className="text-overlay0">{t("Latency")}</dt><dd className="tabular-nums text-subtext1">{status.latencyMs == null ? "—" : `${status.latencyMs.toLocaleString(getAppLocale(), { maximumFractionDigits: 2 })} ms`}</dd>
                    </dl>
                    <button type="button" className="btn btn-ghost btn-xs shrink-0" disabled={pending} onClick={() => void reconnect(status.id)} aria-label={`${t("Reconnect")}: ${status.account} ${status.streamType}`}>
                      <RefreshCw className="h-3.5 w-3.5" />{t("Reconnect")}
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </section>
        <div className="modal-backdrop"><button type="button" aria-label={t("Close")} onClick={close} /></div>
      </Dialog>
    </>
  );
}

function timestamp(value: string | null) {
  if (!value) return "—";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "—";
  const pad = (part: number, width = 2) => String(part).padStart(width, "0");
  return `${pad(date.getFullYear(), 4)}/${pad(date.getMonth() + 1)}/${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}.${pad(date.getMilliseconds(), 3)}`;
}

function stateLabel(state: string) {
  switch (state) {
    case "connected": return t("Connected");
    case "connecting": return t("Connecting");
    case "reconnecting": return t("Reconnecting");
    case "disconnected": return t("Disconnected");
    default: return state;
  }
}
