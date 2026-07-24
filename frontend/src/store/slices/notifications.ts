export function incrementUnreadResources(
  current: Record<string, number>,
  resources: ReadonlySet<string> | ReadonlyMap<string, number>,
) {
  if (resources.size === 0) return current;
  const next = { ...current };
  const entries =
    resources instanceof Map
      ? resources.entries()
      : [...resources].map((resourceId) => [resourceId, 1] as const);
  for (const [resourceId, increment] of entries) {
    next[resourceId] = Math.min(
      Number.MAX_SAFE_INTEGER,
      (next[resourceId] ?? 0) + increment,
    );
  }
  return next;
}

export function clearUnreadResource(
  current: Record<string, number>,
  resourceId: string,
) {
  if ((current[resourceId] ?? 0) === 0) return current;
  return { ...current, [resourceId]: 0 };
}

