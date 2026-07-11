import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./components/App";
import { AppErrorBoundary } from "./components/common/AppErrorBoundary";
import { installConsoleLogForwarding } from "./utils/consoleLogging";
import "perfect-scrollbar/css/perfect-scrollbar.css";
import "./styles.css";

installConsoleLogForwarding();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <AppErrorBoundary>
      <App />
    </AppErrorBoundary>
  </React.StrictMode>,
);
