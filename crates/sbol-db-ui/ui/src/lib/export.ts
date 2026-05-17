/**
 * Result-set export helpers. Build a Blob from the in-memory columns/
 * rows and trigger a download via an invisible `<a>` element. Pure
 * client side — no server roundtrip — so we can offer downloads even
 * after the connection's been closed.
 */

export interface ExportColumn {
  name: string;
}

export type ExportCell = unknown;

export function downloadCsv(
  columns: ExportColumn[],
  rows: ExportCell[][],
  filename: string
): void {
  const header = columns.map((c) => csvEscape(c.name)).join(",");
  const body = rows
    .map((row) => row.map((cell) => csvEscape(toScalar(cell))).join(","))
    .join("\n");
  triggerDownload(`${header}\n${body}\n`, "text/csv;charset=utf-8", filename);
}

export function downloadJson(
  columns: ExportColumn[],
  rows: ExportCell[][],
  filename: string
): void {
  const objects = rows.map((row) => {
    const obj: Record<string, ExportCell> = {};
    columns.forEach((c, i) => {
      obj[c.name] = row[i] ?? null;
    });
    return obj;
  });
  triggerDownload(
    JSON.stringify(objects, null, 2),
    "application/json;charset=utf-8",
    filename
  );
}

function csvEscape(value: unknown): string {
  if (value === null || value === undefined) return "";
  const s = typeof value === "string" ? value : JSON.stringify(value);
  // Wrap in quotes whenever the field contains a delimiter, quote, or
  // newline. Inner quotes are doubled per RFC 4180.
  if (/[",\n\r]/.test(s)) {
    return `"${s.replace(/"/g, '""')}"`;
  }
  return s;
}

function toScalar(value: ExportCell): unknown {
  if (value === null || value === undefined) return "";
  if (
    typeof value === "string" ||
    typeof value === "number" ||
    typeof value === "boolean"
  ) {
    return value;
  }
  return JSON.stringify(value);
}

function triggerDownload(body: string, mime: string, filename: string): void {
  const blob = new Blob([body], { type: mime });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  a.remove();
  // Defer revocation to give the browser a tick to start the download.
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
