import React from "react";
import { Loader2 } from "lucide-react";
import { useAppStore } from "../../store/appStore";
import { invokeTypedCommand } from "../../api/tauri";
import { t } from "../../i18n";

export function LoginView({ cancellable }: { cancellable: boolean }) {
  const loginWithInstanceDomain = useAppStore(
    (state) => state.loginWithInstanceDomain,
  );
  const loginWithBluesky = useAppStore((state) => state.loginWithBluesky);
  const [domain, setDomain] = React.useState("mastodon.social");
  const [identifier, setIdentifier] = React.useState("");
  const [password, setPassword] = React.useState("");
  const [status, setStatus] = React.useState("");
  const [loading, setLoading] = React.useState<"instance" | "bluesky" | null>(
    null,
  );
  const operationRef = React.useRef<string | null>(null);
  const cancellationRequestedRef = React.useRef(false);

  React.useEffect(
    () => () => {
      const targetOperationId = operationRef.current;
      if (targetOperationId) {
        void invokeTypedCommand("cancel_login_flow", {
          request: { targetOperationId },
        });
      }
    },
    [],
  );

  const startInstanceLogin = React.useCallback(async () => {
    const trimmed = domain.trim();
    if (!trimmed || loading) {
      if (!trimmed) setStatus(t("Enter your instance domain to log in"));
      return;
    }
    const operationId = crypto.randomUUID();
    operationRef.current = operationId;
    cancellationRequestedRef.current = false;
    setLoading("instance");
    setStatus(t("Connecting to {domain}...", { domain: trimmed }));
    const ok = await loginWithInstanceDomain(trimmed, operationId);
    if (operationRef.current === operationId) operationRef.current = null;
    if (!ok && !cancellationRequestedRef.current) {
      setLoading(null);
      setStatus(t("Login failed."));
    }
  }, [domain, loading, loginWithInstanceDomain]);

  const startBlueskyLogin = React.useCallback(async () => {
    const trimmedIdentifier = identifier.trim();
    if (!trimmedIdentifier || !password || loading) {
      if (!trimmedIdentifier || !password) {
        setStatus(`${t("Username or email")} / ${t("App password")}`);
      }
      return;
    }
    const operationId = crypto.randomUUID();
    operationRef.current = operationId;
    cancellationRequestedRef.current = false;
    setLoading("bluesky");
    setStatus(t("Connecting to Bluesky..."));
    const ok = await loginWithBluesky(trimmedIdentifier, password, operationId);
    if (operationRef.current === operationId) operationRef.current = null;
    if (!ok && !cancellationRequestedRef.current) {
      setLoading(null);
      setStatus(t("Login failed."));
    }
  }, [identifier, loading, loginWithBluesky, password]);

  const cancel = React.useCallback(() => {
    if (!cancellable) return;
    const targetOperationId = operationRef.current;
    if (targetOperationId) {
      cancellationRequestedRef.current = true;
      setStatus(t("Cancelling..."));
      void invokeTypedCommand("cancel_login_flow", {
        request: { targetOperationId },
      });
    }
    useAppStore.setState({ loginOpen: false });
  }, [cancellable]);

  return (
    <div className="flex h-screen flex-col bg-base-100">
      <div
        className="h-8 shrink-0 border-b border-surface0 bg-base-300"
        data-tauri-drag-region
      />
      <main className="grid min-h-0 flex-1 place-items-center px-6">
        <div className="flex w-full max-w-80 flex-col items-stretch gap-3">
          <h1 className="text-center text-lg font-normal text-text">awayuki</h1>
          <form
            aria-label={t("Instance login")}
            className="flex flex-col items-stretch gap-3"
            onSubmit={(event) => {
              event.preventDefault();
              void startInstanceLogin();
            }}
          >
            <p className="text-center text-sm text-subtext0">
              {t("Enter your instance domain to log in")}
            </p>
            <label className="sr-only" htmlFor="instance-domain">
              {t("Instance domain")}
            </label>
            <input
              id="instance-domain"
              name="instance-domain"
              className="input input-bordered input-sm h-8 min-h-8 border-surface1 bg-base-100 text-sm"
              value={domain}
              onChange={(event) => setDomain(event.target.value)}
              disabled={loading !== null}
              autoCapitalize="none"
              autoComplete="url"
              autoCorrect="off"
              inputMode="url"
              spellCheck={false}
            />
            <button
              type="submit"
              className="btn btn-secondary btn-sm h-8 min-h-8 px-4 text-sm font-normal"
              disabled={loading !== null}
            >
              {loading === "instance" ? (
                <Loader2 className="h-4 w-4 animate-spin" />
              ) : null}
              {t("Log in")}
            </button>
          </form>

          <div className="my-2 flex items-center gap-2 text-sm text-overlay0">
            <div className="h-px flex-1 bg-surface0" />
            {t("or")}
            <div className="h-px flex-1 bg-surface0" />
          </div>

          <form
            aria-label={t("Bluesky login")}
            className="flex flex-col items-stretch gap-3"
            onSubmit={(event) => {
              event.preventDefault();
              void startBlueskyLogin();
            }}
          >
            <div className="text-sm text-subtext0">Bluesky:</div>
            <label className="sr-only" htmlFor="bluesky-identifier">
              {t("Username or email")}
            </label>
            <input
              id="bluesky-identifier"
              name="username"
              className="input input-bordered input-sm h-8 min-h-8 border-surface1 bg-base-100 text-sm"
              placeholder={t("Username or email")}
              value={identifier}
              onChange={(event) => setIdentifier(event.target.value)}
              disabled={loading !== null}
              autoCapitalize="none"
              autoComplete="username"
              autoCorrect="off"
              spellCheck={false}
            />
            <label className="sr-only" htmlFor="bluesky-app-password">
              {t("App password")}
            </label>
            <input
              id="bluesky-app-password"
              name="password"
              className="input input-bordered input-sm h-8 min-h-8 border-surface1 bg-base-100 text-sm"
              placeholder={t("App password")}
              type="password"
              value={password}
              onChange={(event) => setPassword(event.target.value)}
              disabled={loading !== null}
              autoComplete="current-password"
            />
            <button
              type="submit"
              className="btn btn-secondary btn-sm h-8 min-h-8 px-4 text-sm font-normal"
              disabled={loading !== null}
            >
              {loading === "bluesky" ? (
                <Loader2 className="h-4 w-4 animate-spin" />
              ) : null}
              {t("Log in")}
            </button>
          </form>

          {cancellable ? (
            <>
              <div className="mt-2 h-px bg-surface0" />
              <button
                type="button"
                className="btn btn-ghost btn-sm h-8 min-h-8 text-sm font-normal"
                onClick={cancel}
              >
                {t("Cancel")}
              </button>
            </>
          ) : null}

          {status ? (
            <div className="text-center text-sm text-subtext0">{status}</div>
          ) : null}
        </div>
      </main>
    </div>
  );
}
