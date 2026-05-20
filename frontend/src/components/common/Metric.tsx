export function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div className="border-r border-surface0 px-4 py-3 last:border-r-0">
      <div className="text-xs text-subtext0">{label}</div>
      <div className="mt-1 text-lg text-text">{value}</div>
    </div>
  );
}
