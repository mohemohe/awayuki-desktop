import { LruCache } from "./lru";

type NegativeEntry = { retryAt: number };
type Probe = (url: string, signal?: AbortSignal) => Promise<boolean>;
type ProbeFlight = {
  promise: Promise<boolean>;
  consumers: Set<symbol>;
  delayTimer: number | null;
  timeoutTimer: number | null;
  settled: boolean;
  controller: AbortController;
  resolve: (loaded: boolean) => void;
};

const MAX_IN_FLIGHT_PROBES = 256;
const PROBE_TIMEOUT_MS = 15_000;
const inFlight = new Map<string, ProbeFlight>();
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
      delayTimer: null,
      timeoutTimer: null,
      settled: false,
      controller: new AbortController(),
      resolve,
    };
    const scheduledFlight = flight;
    scheduledFlight.delayTimer = window.setTimeout(() => {
      if (scheduledFlight.settled) return;
      scheduledFlight.delayTimer = null;
      scheduledFlight.timeoutTimer = window.setTimeout(() => {
        finishFlight(url, scheduledFlight, false, true);
      }, PROBE_TIMEOUT_MS);
      void probe(url, scheduledFlight.controller.signal)
        .then((loaded) => {
          finishFlight(url, scheduledFlight, loaded, true);
        })
        .catch(() => {
          finishFlight(url, scheduledFlight, false, true);
        });
    }, delay);
    registerFlight(url, scheduledFlight);
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
      if (flight!.consumers.size === 0) {
        finishFlight(url, flight!, false, false);
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
  clearFlights();
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

function browserProbe(url: string, signal?: AbortSignal) {
  return new Promise<boolean>((resolve) => {
    const image = new Image();
    let settled = false;
    const finish = (loaded: boolean) => {
      if (settled) return;
      settled = true;
      signal?.removeEventListener("abort", abort);
      image.onload = null;
      image.onerror = null;
      image.removeAttribute("src");
      resolve(loaded);
    };
    const abort = () => finish(false);
    if (signal?.aborted) {
      abort();
      return;
    }
    signal?.addEventListener("abort", abort, { once: true });
    image.onload = () => finish(true);
    image.onerror = () => finish(false);
    image.src = url;
  });
}

export function setMediaProbeForTesting(nextProbe: Probe) {
  clearFlights();
  probe = nextProbe;
  negativeCache.clear();
  return () => {
    clearFlights();
    probe = browserProbe;
    negativeCache.clear();
  };
}

function registerFlight(url: string, flight: ProbeFlight) {
  if (inFlight.size >= MAX_IN_FLIGHT_PROBES) {
    const oldest = inFlight.entries().next().value as
      [string, ProbeFlight] | undefined;
    if (oldest) finishFlight(oldest[0], oldest[1], false, false);
  }
  inFlight.set(url, flight);
}

function finishFlight(
  url: string,
  flight: ProbeFlight,
  loaded: boolean,
  cacheResult: boolean,
) {
  if (flight.settled) return;
  flight.settled = true;
  if (flight.delayTimer !== null) {
    window.clearTimeout(flight.delayTimer);
    flight.delayTimer = null;
  }
  if (flight.timeoutTimer !== null) {
    window.clearTimeout(flight.timeoutTimer);
    flight.timeoutTimer = null;
  }
  flight.controller.abort();
  if (cacheResult) {
    if (loaded) negativeCache.delete(url);
    else negativeCache.set(url, { retryAt: Date.now() + 10_000 });
  }
  flight.resolve(loaded);
  if (inFlight.get(url) === flight) inFlight.delete(url);
}

function clearFlights() {
  for (const [url, flight] of [...inFlight]) {
    finishFlight(url, flight, false, false);
  }
}
