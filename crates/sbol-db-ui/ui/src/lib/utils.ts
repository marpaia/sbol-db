import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

import { ApiError } from "./api";

/**
 * Shadcn-style className combiner. Resolves Tailwind class conflicts
 * (e.g. `px-2 px-4` → `px-4`) and accepts conditional fragments.
 */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}

/**
 * Best-effort human-readable message for an unknown error. For an
 * `ApiError` returned by `asApiError`, surfaces the `detail` field
 * from the server's JSON problem response when present, falling back
 * to the raw body, then the generic message. Other thrown values
 * stringify naively.
 */
export function describeError(error: unknown): string {
  if (error instanceof ApiError) {
    if (error.body) {
      try {
        const parsed = JSON.parse(error.body) as { detail?: unknown };
        if (typeof parsed.detail === "string" && parsed.detail.length > 0) {
          return parsed.detail;
        }
      } catch {
        // not JSON; fall through to raw body
      }
      return error.body;
    }
    return error.message;
  }
  if (error instanceof Error) return error.message;
  return String(error);
}

/**
 * "9s ago", "4m ago", "2h ago", "3d ago" — short relative time labels
 * for timestamps the UI renders. Accepts ISO strings or Date objects.
 */
export function formatRelative(iso: string | Date | null | undefined): string {
  if (!iso) return "—";
  const then = (typeof iso === "string" ? new Date(iso) : iso).getTime();
  if (Number.isNaN(then)) return "—";
  const seconds = Math.max(0, Math.floor((Date.now() - then) / 1000));
  if (seconds < 60) return `${seconds}s ago`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  return `${Math.floor(seconds / 86400)}d ago`;
}

/**
 * Compact byte sizes: 412 B, 12.4 KB, 1.2 MB, 4.2 GB.
 */
export function formatBytes(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined || Number.isNaN(bytes)) return "—";
  const abs = Math.abs(bytes);
  if (abs < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = abs / 1024;
  let unit = units[0];
  for (let i = 0; i < units.length; i++) {
    if (value < 1024 || i === units.length - 1) {
      unit = units[i];
      break;
    }
    value /= 1024;
  }
  const sign = bytes < 0 ? "-" : "";
  return `${sign}${value < 10 ? value.toFixed(1) : Math.round(value)} ${unit}`;
}

/**
 * Compact uptime: 3d 4h, 12h 5m, 4m 12s, 38s.
 */
export function formatUptime(secs: number | null | undefined): string {
  if (secs === null || secs === undefined || Number.isNaN(secs)) return "—";
  const s = Math.max(0, Math.floor(secs));
  const days = Math.floor(s / 86400);
  const hours = Math.floor((s % 86400) / 3600);
  const mins = Math.floor((s % 3600) / 60);
  const seconds = s % 60;
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${mins}m`;
  if (mins > 0) return `${mins}m ${seconds}s`;
  return `${seconds}s`;
}

/**
 * Compact duration in milliseconds: 18 ms, 1.2 s, 4.5 s, 1.3 m.
 */
export function formatMs(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || Number.isNaN(ms)) return "—";
  if (ms < 1) return `${ms.toFixed(2)} ms`;
  if (ms < 1000) return `${Math.round(ms)} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`;
  return `${(ms / 60_000).toFixed(1)} m`;
}
