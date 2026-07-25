import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./components/App";
import { AppErrorBoundary } from "./components/common/AppErrorBoundary";
import { installConsoleLogForwarding } from "./utils/consoleLogging";
import { markFrontendModuleEvaluated } from "./utils/startupMetrics";
import "perfect-scrollbar/css/perfect-scrollbar.css";
import "./styles.css";

markFrontendModuleEvaluated();
installConsoleLogForwarding();

const root = ReactDOM.createRoot(document.getElementById("root")!);
const render = (application: React.ReactNode) =>
  root.render(
    <React.StrictMode>
      <AppErrorBoundary>{application}</AppErrorBoundary>
    </React.StrictMode>,
  );

declare global {
  interface Window {
    __AWAYUKI_RELEASE_WEBVIEW_SMOKE_URL__?: string;
  }
}

let releaseWebviewSmokeActivated = false;
const activateReleaseWebviewSmoke = (baseUrl: string) => {
  if (releaseWebviewSmokeActivated || !/^http:\/\/(?:127\.0\.0\.1|localhost|\[::1\]):\d+$/.test(baseUrl)) {
    return;
  }
  releaseWebviewSmokeActivated = true;
  void import("./performance/ReleaseWebviewSmokeApp").then(
    ({ ReleaseWebviewSmokeApp }) => render(<ReleaseWebviewSmokeApp baseUrl={baseUrl} />),
    (error) => console.error("AWAYUKI_WEBVIEW_SECURITY_IMPORT_ERROR", error),
  );
};
window.addEventListener("awayuki-release-webview-smoke", (event) => {
  const baseUrl = (event as CustomEvent<unknown>).detail;
  if (typeof baseUrl === "string") activateReleaseWebviewSmoke(baseUrl);
});
if (window.__AWAYUKI_RELEASE_WEBVIEW_SMOKE_URL__) {
  activateReleaseWebviewSmoke(window.__AWAYUKI_RELEASE_WEBVIEW_SMOKE_URL__);
}

if (import.meta.env.VITE_PERFORMANCE_SMOKE === "startup") {
  console.info("AWAYUKI_PERFORMANCE_STARTUP bootstrap");
  void import("./performance/PerformanceStartupApp").then(
    ({ PerformanceStartupApp }) =>
      render(
        <>
          <App />
          <PerformanceStartupApp />
        </>,
      ),
    (error) => {
      console.error("AWAYUKI_PERFORMANCE_STARTUP import failed", error);
      render(<App />);
    },
  );
} else if (import.meta.env.VITE_PERFORMANCE_SMOKE === "1") {
  console.info("AWAYUKI_PERFORMANCE_SMOKE bootstrap");
  void import("./performance/PerformanceSmokeApp").then(
    ({ PerformanceSmokeApp }) => {
      console.info("AWAYUKI_PERFORMANCE_SMOKE imported");
      render(
        <>
          <App />
          <PerformanceSmokeApp />
        </>,
      );
    },
    (error) => {
      console.error("AWAYUKI_PERFORMANCE_SMOKE import failed", error);
      render(<App />);
    },
  );
} else {
  render(<App />);
}
