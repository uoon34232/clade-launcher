// Test helper only (imported by tests, never by the app): an inline PNG with a real PNG header of
// the given size. The pixel data is not a valid image, which is enough for everything that only
// reads the header; `seed` makes the URIs of different "accounts" differ.
export function testSkin(width = 64, height = 64, seed = 0): string {
  const be = (n: number) =>
    String.fromCharCode((n >>> 24) & 255, (n >>> 16) & 255, (n >>> 8) & 255, n & 255);
  const header =
    "\x89PNG\r\n\x1a\n" +
    "\x00\x00\x00\x0d" +
    "IHDR" +
    be(width) +
    be(height) +
    "\x08\x06\x00\x00\x00";
  return `data:image/png;base64,${btoa(header + `fixture-${seed}-pixels`)}`;
}
