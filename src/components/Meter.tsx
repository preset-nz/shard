/**
 * Peak output. Not a VU meter and not calibrated — it exists to answer "is
 * anything coming out" and "am I about to hit the safety clip".
 */
export function Meter({ peak }: { peak: number }) {
  const pct = Math.min(100, peak * 100);
  const hot = peak > 0.95;
  return (
    <span className="flex items-center gap-2">
      <span className="relative h-1.5 w-28 overflow-hidden rounded bg-muted">
        <span
          className={`absolute inset-y-0 left-0 transition-[width] duration-75 ${
            hot ? 'bg-destructive' : 'bg-primary'
          }`}
          style={{ width: `${pct}%` }}
        />
      </span>
      <span className="w-10 tabular-nums">{peak.toFixed(2)}</span>
    </span>
  );
}
