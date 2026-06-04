export const pollDurations = [
  { label: "5m", labelJa: "5 分", seconds: 5 * 60 },
  { label: "30m", labelJa: "30 分", seconds: 30 * 60 },
  { label: "1h", labelJa: "1 時間", seconds: 60 * 60 },
  { label: "6h", labelJa: "6 時間", seconds: 6 * 60 * 60 },
  { label: "12h", labelJa: "12 時間", seconds: 12 * 60 * 60 },
  { label: "1d", labelJa: "1 日", seconds: 24 * 60 * 60 },
  { label: "3d", labelJa: "3 日", seconds: 3 * 24 * 60 * 60 },
  { label: "7d", labelJa: "7 日", seconds: 7 * 24 * 60 * 60 },
];

export function pollDurationLabel(seconds: number) {
  const duration = pollDurations.find((item) => item.seconds === seconds);
  return duration?.label ?? "1d";
}
