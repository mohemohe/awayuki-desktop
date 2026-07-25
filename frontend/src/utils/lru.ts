export type LruCacheOptions<Value> = {
  ttlMs?: number;
  maxWeight?: number;
  weight?: (value: Value) => number;
  now?: () => number;
};

type CacheEntry<Value> = {
  value: Value;
  expiresAt: number;
  weight: number;
};

export class LruCache<Key, Value> {
  private readonly values = new Map<Key, CacheEntry<Value>>();
  private readonly ttlMs: number;
  private readonly maxWeight: number;
  private readonly weigh: (value: Value) => number;
  private readonly now: () => number;
  private currentWeight = 0;

  constructor(
    private readonly capacity: number,
    options: LruCacheOptions<Value> = {},
  ) {
    if (!Number.isInteger(capacity) || capacity < 1) {
      throw new Error("LRU capacity must be a positive integer");
    }
    this.ttlMs = Math.max(0, options.ttlMs ?? Number.POSITIVE_INFINITY);
    this.maxWeight = Math.max(
      1,
      options.maxWeight ?? Number.POSITIVE_INFINITY,
    );
    this.weigh = options.weight ?? (() => 1);
    this.now = options.now ?? Date.now;
  }

  get size() {
    this.sweepExpired();
    return this.values.size;
  }

  get weight() {
    this.sweepExpired();
    return this.currentWeight;
  }

  has(key: Key) {
    return this.validEntry(key) !== undefined;
  }

  get(key: Key) {
    const entry = this.validEntry(key);
    if (!entry) return undefined;
    this.values.delete(key);
    this.values.set(key, entry);
    return entry.value;
  }

  set(key: Key, value: Value) {
    this.delete(key);
    const weight = Math.max(1, Math.ceil(this.weigh(value)));
    if (weight > this.maxWeight) return this;
    const expiresAt = Number.isFinite(this.ttlMs)
      ? this.now() + this.ttlMs
      : Number.POSITIVE_INFINITY;
    this.values.set(key, { value, expiresAt, weight });
    this.currentWeight += weight;
    this.evictToBudget();
    return this;
  }

  delete(key: Key) {
    const entry = this.values.get(key);
    if (!entry) return false;
    this.values.delete(key);
    this.currentWeight = Math.max(0, this.currentWeight - entry.weight);
    return true;
  }

  clear() {
    this.values.clear();
    this.currentWeight = 0;
  }

  sweepExpired() {
    const now = this.now();
    for (const [key, entry] of this.values) {
      if (entry.expiresAt <= now) this.delete(key);
    }
  }

  private validEntry(key: Key) {
    const entry = this.values.get(key);
    if (!entry) return undefined;
    if (entry.expiresAt <= this.now()) {
      this.delete(key);
      return undefined;
    }
    return entry;
  }

  private evictToBudget() {
    while (
      this.values.size > this.capacity ||
      this.currentWeight > this.maxWeight
    ) {
      const oldest = this.values.keys().next().value as Key | undefined;
      if (oldest === undefined) break;
      this.delete(oldest);
    }
  }
}
