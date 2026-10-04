// Neutral placeholder face on an 8x8 grid, shown while an account has no rendered head (see PlayerHead).
const GRID = [
  "HHHHHHHH",
  "HHHHHHHH",
  "HSSSSSSH",
  "SSSSSSSS",
  "SESSSSES",
  "SSSSSSSS",
  "SSMMMMSS",
  "SSSSSSSS",
];
const COLORS: Record<string, string> = {
  H: "#4a3426",
  S: "#d9a784",
  E: "#2b2622",
  M: "#a8604a",
};

export function PixelAvatar({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 8 8" shapeRendering="crispEdges" className={className} aria-hidden>
      {GRID.flatMap((row, y) =>
        [...row].map((c, x) => (
          <rect key={`${x}-${y}`} x={x} y={y} width="1.02" height="1.02" fill={COLORS[c]} />
        )),
      )}
    </svg>
  );
}
