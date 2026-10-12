const UNITS: [Intl.RelativeTimeFormatUnit, number][] = [
  ["year", 365 * 24 * 3600],
  ["month", 30 * 24 * 3600],
  ["week", 7 * 24 * 3600],
  ["day", 24 * 3600],
  ["hour", 3600],
  ["minute", 60],
];
const rtf = new Intl.RelativeTimeFormat("en", { numeric: "auto" });

export function relativeTime(iso: string | null | undefined): string {
  if (!iso) return "";
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "";
  const diff = (t - Date.now()) / 1000;
  for (const [unit, secs] of UNITS) {
    if (Math.abs(diff) >= secs) return rtf.format(Math.round(diff / secs), unit);
  }
  return "just now";
}

export function fullDate(iso: string | null | undefined): string {
  if (!iso) return "";
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? "" : d.toLocaleString();
}

export function shortSha(sha: string): string {
  return sha.slice(0, 7);
}

export function firstLine(msg: string): string {
  return msg.split("\n")[0];
}

export function restLines(msg: string): string {
  const i = msg.indexOf("\n");
  return i < 0 ? "" : msg.slice(i + 1).trim();
}

export function humanSize(n: number): string {
  if (n < 1024) return `${n} Bytes`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

/** Readable text color for a label background (hex without #). */
export function labelTextColor(hex: string): string {
  const h = hex.replace("#", "").padEnd(6, "0");
  const r = parseInt(h.slice(0, 2), 16);
  const g = parseInt(h.slice(2, 4), 16);
  const b = parseInt(h.slice(4, 6), 16);
  return (r * 299 + g * 587 + b * 114) / 1000 > 150 ? "#1f2328" : "#ffffff";
}

/** Encode each path segment for use in a SPA URL. */
export function encPath(p: string): string {
  return p.split("/").filter(Boolean).map(encodeURIComponent).join("/");
}

/**
 * Split a "ref/with/slashes/and/path" splat into {ref, path} using known branch names
 * (longest match wins), falling back to the first segment.
 */
export function splitRefPath(splat: string, refs: string[]): { ref: string; path: string } {
  const s = splat.replace(/^\/+/, "");
  const match = refs
    .filter((r) => s === r || s.startsWith(r + "/"))
    .sort((a, b) => b.length - a.length)[0];
  if (match) return { ref: match, path: s.slice(match.length + 1) };
  const i = s.indexOf("/");
  return i < 0 ? { ref: s, path: "" } : { ref: s.slice(0, i), path: s.slice(i + 1) };
}

export function host(): string {
  return window.location.host;
}

export function origin(): string {
  return window.location.origin;
}
