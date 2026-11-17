// Small building blocks shared by the release tools: hashing, a minimal and deterministic ZIP writer
// and reader (no dependency, because the archive layout and contents are a security boundary that
// should be easy to audit), and path helpers. Pure Node, no network.

import { createHash } from "node:crypto";
import { readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { join, relative, sep } from "node:path";
import { crc32, deflateRawSync, inflateRawSync } from "node:zlib";

export const PACKAGE_ROOT = "Clade-Launcher";
export const ZIP_NAME = "Clade-Launcher-Portable.zip";

export const sha256Hex = (data) => createHash("sha256").update(data).digest("hex");
export const sha256File = (path) => sha256Hex(readFileSync(path));

/** `<hex>  <name>\n`, the format `sha256sum -c` and PowerShell's Get-FileHash comparison use. */
export const sha256Line = (hex, name) => `${hex}  ${name}\n`;

/** Parses a `.sha256` file. Returns `{ hex, name }` or null if it is not in the expected shape. */
export function parseSha256File(text) {
  const m = /^([0-9a-fA-F]{64}) [ *]([^\r\n]+)\r?\n?$/.exec(text);
  return m ? { hex: m[1].toLowerCase(), name: m[2] } : null;
}

/** All files below `dir`, as POSIX-style relative paths, sorted bytewise (so output is stable). */
export function listFiles(dir) {
  const out = [];
  const walk = (d) => {
    for (const e of readdirSync(d, { withFileTypes: true })) {
      const p = join(d, e.name);
      if (e.isDirectory()) walk(p);
      else if (e.isFile()) out.push(relative(dir, p).split(sep).join("/"));
    }
  };
  walk(dir);
  return out.sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
}

// ------------------------------------------------------------------------------------- ZIP

const DOS_EPOCH_YEAR = 1980;

/** DOS date and time for a Unix timestamp (UTC). Fixed by default, so archives do not depend on when
 *  they were built. */
export function dosDateTime(unixSeconds) {
  const d = new Date(unixSeconds * 1000);
  const year = Math.max(d.getUTCFullYear(), DOS_EPOCH_YEAR);
  const date = ((year - DOS_EPOCH_YEAR) << 9) | ((d.getUTCMonth() + 1) << 5) | d.getUTCDate();
  const time = (d.getUTCHours() << 11) | (d.getUTCMinutes() << 5) | (d.getUTCSeconds() >> 1);
  return { date, time };
}

/**
 * Builds a ZIP from `[{ name, data: Buffer }]`. Entries are written in bytewise name order with a
 * fixed timestamp and fixed attributes, so the same inputs always give the same bytes. No ZIP64:
 * a package that needs it is far outside what this release model expects, and is refused.
 */
export function buildZip(entries, { epoch = 315532800 } = {}) {
  const sorted = [...entries].sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
  const names = new Set();
  const { date, time } = dosDateTime(epoch);
  const parts = [];
  const central = [];
  let offset = 0;
  for (const { name, data } of sorted) {
    if (names.has(name)) throw new Error(`duplicate entry: ${name}`);
    names.add(name);
    if (name.startsWith("/") || name.includes("\\") || name.split("/").includes(".."))
      throw new Error(`unsafe entry name: ${name}`);
    const nameBuf = Buffer.from(name, "utf8");
    const crc = crc32(data);
    const deflated = data.length > 0 ? deflateRawSync(data, { level: 9 }) : Buffer.alloc(0);
    const useDeflate = data.length > 0 && deflated.length < data.length;
    const body = useDeflate ? deflated : data;
    if (body.length >= 0xffffffff || data.length >= 0xffffffff || offset >= 0xffffffff)
      throw new Error("the archive would need ZIP64, which is not supported");
    const header = Buffer.alloc(30);
    header.writeUInt32LE(0x04034b50, 0);
    header.writeUInt16LE(20, 4); // version needed
    header.writeUInt16LE(0x0800, 6); // UTF-8 names
    header.writeUInt16LE(useDeflate ? 8 : 0, 8);
    header.writeUInt16LE(time, 10);
    header.writeUInt16LE(date, 12);
    header.writeUInt32LE(crc, 14);
    header.writeUInt32LE(body.length, 18);
    header.writeUInt32LE(data.length, 22);
    header.writeUInt16LE(nameBuf.length, 26);
    header.writeUInt16LE(0, 28);
    parts.push(header, nameBuf, body);
    const cd = Buffer.alloc(46);
    cd.writeUInt32LE(0x02014b50, 0);
    cd.writeUInt16LE(20, 4); // made by (MS-DOS, 2.0)
    cd.writeUInt16LE(20, 6);
    cd.writeUInt16LE(0x0800, 8);
    cd.writeUInt16LE(useDeflate ? 8 : 0, 10);
    cd.writeUInt16LE(time, 12);
    cd.writeUInt16LE(date, 14);
    cd.writeUInt32LE(crc, 16);
    cd.writeUInt32LE(body.length, 20);
    cd.writeUInt32LE(data.length, 24);
    cd.writeUInt16LE(nameBuf.length, 28);
    cd.writeUInt32LE(0x20, 38); // DOS "archive" attribute
    cd.writeUInt32LE(offset, 42);
    central.push(cd, nameBuf);
    offset += header.length + nameBuf.length + body.length;
  }
  const cdSize = central.reduce((n, b) => n + b.length, 0);
  if (sorted.length >= 0xffff)
    throw new Error("the archive would need ZIP64, which is not supported");
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(sorted.length, 8);
  end.writeUInt16LE(sorted.length, 10);
  end.writeUInt32LE(cdSize, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...parts, ...central, end]);
}

function readZipMeta(buf) {
  if (buf.length < 22) throw new Error("not a ZIP file (no end-of-directory record)");
  let eocd = -1;
  for (let i = buf.length - 22; i >= Math.max(0, buf.length - 22 - 0xffff); i--) {
    if (buf.readUInt32LE(i) === 0x06054b50) {
      eocd = i;
      break;
    }
  }
  if (eocd < 0) throw new Error("not a ZIP file (no end-of-directory record)");
  const count = buf.readUInt16LE(eocd + 10);
  const cdSize = buf.readUInt32LE(eocd + 12);
  const cdOffset = buf.readUInt32LE(eocd + 16);
  if (count === 0xffff || cdOffset === 0xffffffff || cdSize === 0xffffffff)
    throw new Error("ZIP64 archives are not accepted");
  if (cdOffset > eocd || cdOffset + cdSize > eocd) throw new Error("corrupt ZIP central directory");
  let p = cdOffset;
  const entries = [];
  for (let i = 0; i < count; i++) {
    if (p + 46 > eocd || buf.readUInt32LE(p) !== 0x02014b50)
      throw new Error("corrupt ZIP central directory");
    const flags = buf.readUInt16LE(p + 8);
    const method = buf.readUInt16LE(p + 10);
    const crc = buf.readUInt32LE(p + 16);
    const compressedSize = buf.readUInt32LE(p + 20);
    const size = buf.readUInt32LE(p + 24);
    const nameLen = buf.readUInt16LE(p + 28);
    const extraLen = buf.readUInt16LE(p + 30);
    const commentLen = buf.readUInt16LE(p + 32);
    const externalAttr = buf.readUInt32LE(p + 38);
    const offset = buf.readUInt32LE(p + 42);
    if (p + 46 + nameLen + extraLen + commentLen > eocd)
      throw new Error("corrupt ZIP central directory");
    const name = buf.toString(flags & 0x0800 ? "utf8" : "latin1", p + 46, p + 46 + nameLen);
    entries.push({ name, flags, method, crc, size, compressedSize, externalAttr, offset });
    p += 46 + nameLen + extraLen + commentLen;
  }
  if (p !== cdOffset + cdSize) throw new Error("corrupt ZIP central directory");
  return { entries, cdOffset };
}

/** Lists the entries of a ZIP held in memory: `[{ name, flags, method, crc, size, compressedSize, externalAttr, offset }]`. */
export function readZipDirectory(buf) {
  return readZipMeta(buf).entries;
}

/** The content of one entry (stored or deflated). */
export function readZipEntry(buf, entry) {
  const p = entry.offset;
  if (p + 30 > buf.length || buf.readUInt32LE(p) !== 0x04034b50)
    throw new Error(`corrupt local header for ${entry.name}`);
  const start = p + 30 + buf.readUInt16LE(p + 26) + buf.readUInt16LE(p + 28);
  if (start + entry.compressedSize > buf.length)
    throw new Error(`truncated entry data for ${entry.name}`);
  const raw = buf.subarray(start, start + entry.compressedSize);
  if (entry.method === 0) return Buffer.from(raw);
  if (entry.method === 8) return inflateRawSync(raw);
  throw new Error(`unsupported compression method ${entry.method} for ${entry.name}`);
}

export const readZipFile = (path) => {
  const buf = readFileSync(path);
  const { entries, cdOffset } = readZipMeta(buf);
  return { buf, entries, cdOffset };
};

export function writeFileLf(path, text) {
  writeFileSync(path, text.replace(/\r\n/g, "\n"), "utf8");
}

export const fileSize = (p) => statSync(p).size;
