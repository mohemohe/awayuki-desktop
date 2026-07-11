import { LruCache } from "./lru";

type NegativeEntry = { retryAt: number };
type Probe = (url: string) => Promise<boolean>;

const inFlight = new LruCache<string, Promise<boolean>>(256, {
  ttlMs: 60_000,
});
const negativeCache = new LruCache<string, NegativeEntry>(512, {
  ttlMs: 5 * 60_000,
});
let probe: Probe = browserProbe;

export function scheduleMediaProbe(url: string, delayMs: number) {
  const existing = inFlight.get(url);
  if (existing) return existing;

  const retryAt = negativeCache.get(url)?.retryAt ?? 0;
  const delay = Math.max(delayMs, retryAt - Date.now(), 0) + stableJitter(url);
  const flight = new Promise<boolean>((resolve) => {
    window.setTimeout(() => {
      void probe(url)
        .then((loaded) => {
          if (loaded) {
            negativeCache.delete(url);
          } else {
            negativeCache.set(url, { retryAt: Date.now() + 10_000 });
          }
          resolve(loaded);
        })
        .catch(() => {
          negativeCache.set(url, { retryAt: Date.now() + 10_000 });
          resolve(false);
        });
    }, delay);
  }).finally(() => {
    inFlight.delete(url);
  });
  inFlight.set(url, flight);
  return flight;
}

export function recordMediaLoad(url: string | null) {
  if (url) negativeCache.delete(url);
}

export function clearMediaRetryCache() {
  negativeCache.clear();
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
