import React from "react";
import { t } from "../../i18n";

type AppErrorBoundaryState = {
  error: Error | null;
  diagnostics: string;
  copied: boolean;
};

export class AppErrorBoundary extends React.Component<
  React.PropsWithChildren,
  AppErrorBoundaryState
> {
  state: AppErrorBoundaryState = {
    error: null,
    diagnostics: "",
    copied: false,
  };

  static getDerivedStateFromError(error: unknown): AppErrorBoundaryState {
    const normalized =
      error instanceof Error ? error : new Error("Unknown rendering error");
    return {
      error: normalized,
      diagnostics: formatDiagnostics(normalized),
      copied: false,
    };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo) {
    const diagnostics = formatDiagnostics(error, info.componentStack ?? "");
    this.setState({ diagnostics });
    console.error("[awayuki][ui] uncaught render error", error, info);
  }

  private copyDiagnostics = async () => {
    if (!navigator.clipboard) return;
    try {
      await navigator.clipboard.writeText(this.state.diagnostics);
      this.setState({ copied: true });
    } catch (error) {
      console.error("[awayuki][ui] failed to copy diagnostics", error);
    }
  };

  render() {
    if (!this.state.error) return this.props.children;

    return (
      <main className="grid h-screen place-items-center bg-base-100 px-6 text-base-content">
        <section
          className="flex max-w-md flex-col items-center gap-3 text-center"
          role="alert"
        >
          <h1 className="text-lg text-text">
            {t("Awayuki encountered an unexpected UI error")}
          </h1>
          <p className="text-sm text-subtext0">
            {t("Reload the application to recover. You can copy diagnostics before reloading.")}
          </p>
          <div className="mt-2 flex gap-2">
            <button
              type="button"
              className="btn btn-secondary btn-sm h-8 min-h-8 px-4 text-sm font-normal"
              onClick={() => window.location.reload()}
            >
              {t("Reload")}
            </button>
            <button
              type="button"
              className="btn btn-ghost btn-sm h-8 min-h-8 px-4 text-sm font-normal"
              disabled={!navigator.clipboard}
              onClick={() => void this.copyDiagnostics()}
            >
              {this.state.copied ? t("Copied") : t("Copy diagnostics")}
            </button>
          </div>
        </section>
      </main>
    );
  }
}

function formatDiagnostics(error: Error, componentStack = "") {
  return [
    `Awayuki UI error at ${new Date().toISOString()}`,
    error.stack ?? error.message,
    componentStack,
  ]
    .filter(Boolean)
    .join("\n\n");
}
