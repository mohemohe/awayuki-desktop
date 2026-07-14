import { LruCache } from "./lru";

type NegativeEntry = { retryAt: number };
type Probe = (url: string) => Promise<boolean>;
type ProbeFlight = {
  promise: Promise<boolean>;
  consumers: Set<symbol>;
  timer: number;
  started: boolean;
  resolve: (loaded: boolean) => void;
};

const inFlight = new LruCache<string, ProbeFlight>(256, {
  ttlMs: 60_000,
});
const negativeCache = new LruCache<string, NegativeEntry>(512, {
  ttlMs: 5 * 60_000,
});
let probe: Probe = browserProbe;

export function scheduleMediaProbe(
  url: string,
  delayMs: number,
  signal?: AbortSignal,
) {
  const consumer = Symbol(url);
  let flight = inFlight.get(url);
  if (!flight) {
    const retryAt = negativeCache.get(url)?.retryAt ?? 0;
    const delay = Math.max(delayMs, retryAt - Date.now(), 0) + stableJitter(url);
    let resolve!: (loaded: boolean) => void;
    const promise = new Promise<boolean>((nextResolve) => {
      resolve = nextResolve;
    });
    flight = {
      promise,
      consumers: new Set(),
      timer: 0,
      started: false,
      resolve,
    };
    const scheduledFlight = flight;
    scheduledFlight.timer = window.setTimeout(() => {
      scheduledFlight.started = true;
      void probe(url)
        .then((loaded) => {
          if (loaded) {
            negativeCache.delete(url);
          } else {
            negativeCache.set(url, { retryAt: Date.now() + 10_000 });
          }
          scheduledFlight.resolve(loaded);
        })
        .catch(() => {
          negativeCache.set(url, { retryAt: Date.now() + 10_000 });
          scheduledFlight.resolve(false);
        })
        .finally(() => {
          inFlight.delete(url);
        });
    }, delay);
    inFlight.set(url, scheduledFlight);
  }

  flight.consumers.add(consumer);
  return new Promise<boolean>((resolve) => {
    let settled = false;
    const finish = (loaded: boolean) => {
      if (settled) return;
      settled = true;
      signal?.removeEventListener("abort", abort);
      flight!.consumers.delete(consumer);
      resolve(loaded);
    };
    const abort = () => {
      finish(false);
      if (flight!.consumers.size === 0 && !flight!.started) {
        window.clearTimeout(flight!.timer);
        inFlight.delete(url);
        flight!.resolve(false);
      }
    };
    if (signal?.aborted) {
      abort();
      return;
    }
    signal?.addEventListener("abort", abort, { once: true });
    void flight!.promise.then(finish);
  });
}

export function recordMediaLoad(url: string | null) {
  if (url) negativeCache.delete(url);
}

export function clearMediaRetryCache() {
  // Pending flights without mounted consumers self-cancel through their
  // AbortSignals. Cache clearing prevents their result from extending retry
  // suppression into a new account lifecycle.
  negativeCache.clear();
}

export function mediaRetryCacheSnapshot() {
  return {
    inFlight: inFlight.size,
    negative: negativeCache.size,
  };
}

function stableJitter(url: string) {
  let hash = 0;
  for (let index = 0; index < url.length; index += 1) {
    hash = (hash * 31 + url.charCodeAt(index)) >>> 0;
  }
  return hash % 251;
}

function browserProbe(url: string) {
  return new Promise<boolean>((resolve) => {
    const image = new Image();
    image.onload = () => resolve(true);
    image.onerror = () => resolve(false);
    image.src = url;
  });
}

export function setMediaProbeForTesting(nextProbe: Probe) {
  probe = nextProbe;
  inFlight.clear();
  negativeCache.clear();
  return () => {
    probe = browserProbe;
    inFlight.clear();
    negativeCache.clear();
  };
}
