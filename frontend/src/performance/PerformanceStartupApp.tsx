import React from "react";

import { frontendStartupMetricsSnapshot } from "../utils/startupMetrics";

export function PerformanceStartupApp() {
  const requestedAtRef = React.useRef<number | null>(null);
  const visibilityWaitMsRef = React.useRef(0);
  const startedRef = React.useRef(false);
  const emittedRef = React.useRef(false);

  React.useEffect(() => {
    requestedAtRef.current = performance.now();
    let pollTimer: number | undefined;

    const poll = () => {
      if (emittedRef.current || document.hidden) return;
      if (!startedRef.current) {
        startedRef.current = true;
        visibilityWaitMsRef.current = Math.round(
          performance.now() - (requestedAtRef.current ?? performance.now()),
        );
      }
      const startup = frontendStartupMetricsSnapshot();
      if (startup.firstReactCommitMs === 0 || startup.firstInteractiveMs === 0) {
        pollTimer = window.setTimeout(poll, 20);
        return;
      }
      emittedRef.current = true;
      console.info(
        `AWAYUKI_PERFORMANCE_REPORT ${JSON.stringify({
          schemaVersion: 1,
          fixtureId: "awayuki-webview-startup-v1",
          startup,
          stream: { visibilityWaitMs: visibilityWaitMsRef.current },
          userAgent: navigator.userAgent,
        })}`,
      );
    };

    poll();
    document.addEventListener("visibilitychange", poll);
    return () => {
      document.removeEventListener("visibilitychange", poll);
      if (pollTimer !== undefined) window.clearTimeout(pollTimer);
    };
  }, []);

  return null;
}
