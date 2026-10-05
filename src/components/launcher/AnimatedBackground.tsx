// Soft drifting colour fields plus a static grain layer. Only transforms animate,
// so the GPU just moves a few pre-rasterised layers.
const GRAIN =
  "url(\"data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' width='220' height='220'><filter id='n'><feTurbulence type='fractalNoise' baseFrequency='0.85' numOctaves='2' stitchTiles='stitch'/><feColorMatrix values='0 0 0 0 0.17  0 0 0 0 0.15  0 0 0 0 0.13  0 0 0 0.9 0'/></filter><rect width='100%' height='100%' filter='url(%23n)'/></svg>\")";

interface Field {
  size: string;
  position: React.CSSProperties;
  color: string;
  /** Half-travel on each axis in vmax, and the one-way duration in seconds. */
  x: number;
  y: number;
  xDur: number;
  yDur: number;
  scale: [number, number];
  /** One-way duration of a slow opacity swell, for the warm accent only. */
  breathe?: number;
}

// Ordered back to front. Nearer fields are smaller and travel further and faster,
// which reads as parallax. All periods are unrelated so the picture never repeats.
const FIELDS: Field[] = [
  {
    size: "92vmax",
    position: { left: "4vmax", bottom: "-52vmax" },
    color: "rgb(228 213 190 / 0.95)",
    x: 14,
    y: 9,
    xDur: 30,
    yDur: 37,
    scale: [1, 1.1],
  },
  {
    size: "82vmax",
    position: { left: "-30vmax", top: "-40vmax" },
    color: "rgb(233 196 168 / 0.95)",
    x: 40,
    y: 22,
    xDur: 24,
    yDur: 29,
    scale: [0.94, 1.14],
  },
  {
    size: "72vmax",
    position: { right: "-34vmax", top: "-18vmax" },
    color: "rgb(221 184 172 / 0.85)",
    x: 36,
    y: 26,
    xDur: 27,
    yDur: 21,
    scale: [1.1, 0.94],
  },
  {
    // Warm clay accent: sweeps across the whole width, between hero and dock.
    size: "50vmax",
    position: { left: "calc(50% - 25vmax)", top: "6vmax" },
    color: "rgb(196 122 98 / 0.34)",
    x: 46,
    y: 14,
    xDur: 19,
    yDur: 25,
    scale: [0.92, 1.18],
    breathe: 17,
  },
];

export function AnimatedBackground() {
  return (
    <div aria-hidden className="page-base pointer-events-none absolute inset-0 overflow-hidden">
      {FIELDS.map((f, i) => (
        <div
          key={i}
          className="bg-sway"
          style={
            {
              width: f.size,
              height: f.size,
              ...f.position,
              "--x": `${f.x}vmax`,
              animationDuration: `${f.xDur}s`,
              // Negative delay starts each field mid-journey so the first frame is already composed.
              animationDelay: `${-f.xDur * (0.3 + i * 0.17)}s`,
            } as React.CSSProperties
          }
        >
          <div
            className="bg-bob"
            style={
              {
                background: `radial-gradient(closest-side, ${f.color}, transparent)`,
                "--y": `${f.y}vmax`,
                "--s0": f.scale[0],
                "--s1": f.scale[1],
                animationName: f.breathe ? "sway-y, breathe" : "sway-y",
                animationDuration: f.breathe ? `${f.yDur}s, ${f.breathe}s` : `${f.yDur}s`,
                animationDelay: `${-f.yDur * (0.6 - i * 0.11)}s`,
              } as React.CSSProperties
            }
          />
        </div>
      ))}
      {/* Quiet graphite weight at the base to ground the dock */}
      <div
        className="absolute inset-x-0 bottom-0 h-[45%]"
        style={{
          background:
            "radial-gradient(70% 100% at 50% 100%, rgb(43 38 34 / 0.13), transparent 70%)",
        }}
      />
      <div className="absolute inset-0 opacity-[0.07]" style={{ backgroundImage: GRAIN }} />
    </div>
  );
}
