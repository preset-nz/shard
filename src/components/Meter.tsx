/**
 * Peak output. Not a VU meter and not calibrated — it exists to answer "is
 * anything coming out" and "is the limiter working". Red now means the
 * limiter is off and the safety clip is engaging; with it on, the amber
 * figure says how far it is holding the peaks down.
 */
export function Meter({ peak, reduction }: { peak: number; reduction: number }) {
  const pct = Math.min(100, peak * 100);
  const hot = peak >= 0.999;
  const db = reduction < 0.999 ? 20 * Math.log10(reduction) : 0;
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
      <span
        className={`w-14 tabular-nums ${db < 0 ? 'text-amber-500' : 'opacity-0'}`}
        title="How far the limiter is holding the peaks down"
      >
        {db < 0 ? `${db.toFixed(1)} dB` : '0.0 dB'}
      </span>
    </span>
  );
}
