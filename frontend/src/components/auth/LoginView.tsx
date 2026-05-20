import React from "react";
import { Loader2 } from "lucide-react";
import { useAppStore } from "../../store/appStore";
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

  const startInstanceLogin = React.useCallback(async () => {
    const trimmed = domain.trim();
    if (!trimmed || loading) {
      if (!trimmed) setStatus(t("Enter your instance domain to log in"));
      return;
    }
    setLoading("instance");
    setStatus(t("Connecting to {domain}...", { domain: trimmed }));
    const ok = await loginWithInstanceDomain(trimmed);
    if (!ok) {
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
    setLoading("bluesky");
    setStatus(t("Connecting to Bluesky..."));
    const ok = await loginWithBluesky(trimmedIdentifier, password);
    if (!ok) {
      setLoading(null);
      setStatus(t("Login failed."));
    }
  }, [identifier, loading, loginWithBluesky, password]);

  const cancel = React.useCallback(() => {
    if (!cancellable || loading) return;
    useAppStore.setState({ loginOpen: false });
  }, [cancellable, loading]);

  return (
    <div className="flex h-screen flex-col bg-base-100">
      <div
        className="h-8 shrink-0 border-b border-surface0 bg-base-300"
        data-tauri-drag-region
      />
      <main className="grid min-h-0 flex-1 place-items-center px-6">
        <form
          className="flex w-full max-w-80 flex-col items-stretch gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            void startInstanceLogin();
          }}
        >
          <h1 className="text-center text-lg font-normal text-text">awayuki</h1>
          <p className="text-center text-sm text-subtext0">
            {t("Enter your instance domain to log in")}
          </p>
          <input
            className="input input-bordered input-sm h-8 min-h-8 border-surface1 bg-base-100 text-sm"
            value={domain}
            onChange={(event) => setDomain(event.target.value)}
            disabled={loading !== null}
            autoCapitalize="none"
            autoCorrect="off"
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

          <div className="my-2 flex items-center gap-2 text-sm text-overlay0">
            <div className="h-px flex-1 bg-surface0" />
            {t("or")}
            <div className="h-px flex-1 bg-surface0" />
          </div>

          <div className="text-sm text-subtext0">Bluesky:</div>
          <input
            className="input input-bordered input-sm h-8 min-h-8 border-surface1 bg-base-100 text-sm"
            placeholder={t("Username or email")}
            value={identifier}
            onChange={(event) => setIdentifier(event.target.value)}
            disabled={loading !== null}
            autoCapitalize="none"
            autoCorrect="off"
            spellCheck={false}
          />
          <input
            className="input input-bordered input-sm h-8 min-h-8 border-surface1 bg-base-100 text-sm"
            placeholder={t("App password")}
            type="password"
            value={password}
            onChange={(event) => setPassword(event.target.value)}
            disabled={loading !== null}
          />
          <button
            type="button"
            className="btn btn-secondary btn-sm h-8 min-h-8 px-4 text-sm font-normal"
            disabled={loading !== null}
            onClick={() => void startBlueskyLogin()}
          >
            {loading === "bluesky" ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : null}
            {t("Log in")}
          </button>

          {cancellable ? (
            <>
              <div className="mt-2 h-px bg-surface0" />
              <button
                type="button"
                className="btn btn-ghost btn-sm h-8 min-h-8 text-sm font-normal"
                disabled={loading !== null}
                onClick={cancel}
              >
                {t("Cancel")}
              </button>
            </>
          ) : null}

          {status ? (
            <div className="text-center text-sm text-subtext0">{status}</div>
          ) : null}
        </form>
      </main>
    </div>
  );
}
