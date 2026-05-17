/**
 * Virtualized result-set table built on TanStack Table + TanStack
 * Virtual. Accepts columns and rows of arbitrary JSON-serializable
 * shape; cell rendering coerces non-string types into readable forms.
 *
 * Layout notes:
 *   - Rows are `display: flex` (not `display: table-row`) so each cell
 *     gets an explicit pixel width. This is what keeps columns aligned
 *     across the absolutely-positioned virtual rows — `table-row` is
 *     stripped of column-layout semantics the moment a row gets
 *     `position: absolute`, which is how virtualization works.
 *   - Column widths come from TanStack's sizing model so the user can
 *     drag the right edge of any header to resize.
 *   - Sticky header is a sibling of the virtual list, not inside the
 *     scrollable column — keeps it pinned without paint glitches.
 */

import { useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  flexRender,
  getCoreRowModel,
  getFilteredRowModel,
  getSortedRowModel,
  useReactTable,
  type ColumnDef,
  type ColumnSizingState,
  type SortingState,
} from "@tanstack/react-table";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ArrowDown, ArrowUp, ChevronsUpDown, Search, X } from "lucide-react";

import { IriContextMenu } from "./IriContextMenu";
import { cn } from "@/lib/utils";

export interface ResultColumn {
  name: string;
  /** Free-form type label rendered in the header (e.g. `text`, `uri`). */
  typeHint?: string;
}

export type ResultCell = string | number | boolean | null | object;
export type ResultRow = ResultCell[];

export interface ResultsTableProps {
  columns: ResultColumn[];
  rows: ResultRow[];
}

interface ColumnMeta {
  typeHint?: string;
  numeric: boolean;
}

// Lowercased Postgres type hints that should be right-aligned. We
// match as a substring (e.g. `int4`, `bigint`, `numeric`, `float8`).
const NUMERIC_HINT = /int|num|dec|float|real|double|serial/i;

function detectNumeric(
  typeHint: string | undefined,
  rows: ResultRow[],
  colIndex: number
): boolean {
  if (typeHint && NUMERIC_HINT.test(typeHint)) return true;
  // SPARQL bindings come through with typeHint="binding" — fall back
  // to peeking at the first few non-null cells.
  let seen = 0;
  for (let i = 0; i < rows.length && seen < 30; i++) {
    const v = rows[i][colIndex];
    if (v === null || v === undefined) continue;
    seen++;
    if (typeof v !== "number") return false;
  }
  return seen > 0;
}

/**
 * Pick default widths for each column. Two passes:
 *
 *  1. Content pass — for each column, take the max of the header
 *     length and the longest cell in the first SAMPLE rows, capped at
 *     MAX_MEASURE_CHARS so a single 200-char IRI doesn't blow out the
 *     layout. Convert chars to pixels (rough monospace estimate) and
 *     clamp to [MIN, NATURAL_MAX_*]. Numeric columns get a tighter
 *     cap since numbers rarely need much width.
 *
 *  2. Fill pass — if the sum of content widths is less than the
 *     available container width, iteratively distribute the slack
 *     across non-numeric columns *proportionally to their content
 *     width*, so naturally-wide columns absorb more space than
 *     short ones. No column is ever pushed past FINAL_MAX, so a
 *     single text column among many numeric columns can't balloon
 *     to take over the table. When all expandable columns hit the
 *     cap, leftover slack stays as empty space on the right —
 *     that's preferable to one column dominating the layout.
 *
 * Returns one size in pixels per input column, in order.
 */
function computeColumnSizes(
  columns: ResultColumn[],
  rows: ResultRow[],
  numericCols: boolean[],
  containerWidth: number
): number[] {
  // Pixel approximations for our font + layout. Monospace at 14px is
  // roughly 8px/glyph; the type hint badge is a 10px uppercase font
  // (narrower). The sort indicator is a 12px chevron plus its gap.
  const NAME_CHAR_PX = 8;
  const HINT_CHAR_PX = 6;
  const SORT_ICON_PX = 16;
  const HEADER_GAPS_PX = 12;
  const CELL_CHAR_PX = 8;
  const HORIZONTAL_PADDING = 28; // px-3 both sides + border + slop
  const MIN = 80;
  const NATURAL_MAX_TEXT = 360;
  const NATURAL_MAX_NUMERIC = 140;
  const FINAL_MAX = 540;
  const MAX_MEASURE_CHARS = 50;
  const SAMPLE = 100;

  // The full header needs to fit the column name, the optional type
  // badge, the sort indicator, padding, and a bit of gap between
  // them. We treat this as a *minimum* — natural-content caps don't
  // get to clip the column name, otherwise the header gets ellipsed
  // and the user can't tell what they're looking at.
  function headerWidth(col: ResultColumn): number {
    const nameWidth = col.name.length * NAME_CHAR_PX;
    const hintWidth = col.typeHint ? col.typeHint.length * HINT_CHAR_PX : 0;
    return (
      nameWidth +
      hintWidth +
      SORT_ICON_PX +
      HEADER_GAPS_PX +
      HORIZONTAL_PADDING
    );
  }

  // Pass 1: content-based natural sizing, with a hard floor at the
  // header width so the column label is always readable.
  const contentSizes = columns.map((col, i) => {
    let maxContentLen = 0;
    const limit = Math.min(rows.length, SAMPLE);
    for (let r = 0; r < limit; r++) {
      const v = rows[r][i];
      if (v === null || v === undefined) continue;
      const s = stringifyForMeasure(v);
      const len = Math.min(s.length, MAX_MEASURE_CHARS);
      if (len > maxContentLen) maxContentLen = len;
    }
    const contentPx = maxContentLen * CELL_CHAR_PX + HORIZONTAL_PADDING;
    const cap = numericCols[i] ? NATURAL_MAX_NUMERIC : NATURAL_MAX_TEXT;
    const cappedContent = Math.min(cap, contentPx);
    // Floor at header width — the natural-content cap is for *cell*
    // sizing, never for clipping the column label.
    return Math.max(MIN, headerWidth(col), cappedContent);
  });

  const total = contentSizes.reduce((a, b) => a + b, 0);
  if (total >= containerWidth || total === 0) return contentSizes;

  // Pass 2: iterative proportional slack distribution to non-numeric
  // columns, capped at FINAL_MAX. If every column is numeric we have
  // to grow them anyway (no other targets exist).
  const sizes = [...contentSizes];
  const eligible = new Set<number>(
    numericCols.map((n, i) => (n ? -1 : i)).filter((i) => i >= 0)
  );
  if (eligible.size === 0) {
    columns.forEach((_, i) => eligible.add(i));
  }

  let remaining = containerWidth - total;
  // Bounded loop — at most one iteration per column since each
  // iteration either fully distributes slack or caps at least one
  // column (removing it from the next pass).
  for (let pass = 0; pass < columns.length && remaining > 1; pass++) {
    const expandable = [...eligible].filter((i) => sizes[i] < FINAL_MAX);
    if (expandable.length === 0) break;
    const expandableTotal = expandable.reduce(
      (s, i) => s + contentSizes[i],
      0
    );
    let distributed = 0;
    for (const i of expandable) {
      const share =
        expandableTotal > 0
          ? contentSizes[i] / expandableTotal
          : 1 / expandable.length;
      const want = remaining * share;
      const grow = Math.min(want, FINAL_MAX - sizes[i]);
      sizes[i] += grow;
      distributed += grow;
    }
    if (distributed < 1) break;
    remaining -= distributed;
  }

  return sizes.map((s) => Math.round(s));
}

function stringifyForMeasure(v: unknown): string {
  if (v === null || v === undefined) return "";
  if (typeof v === "string") return v;
  if (typeof v === "number") {
    return Number.isInteger(v) ? v.toLocaleString() : String(v);
  }
  if (typeof v === "boolean") return String(v);
  try {
    return JSON.stringify(v);
  } catch {
    return String(v);
  }
}

export function ResultsTable({ columns, rows }: ResultsTableProps) {
  const [menu, setMenu] = useState<{
    x: number;
    y: number;
    iri: string;
  } | null>(null);
  const [globalFilter, setGlobalFilter] = useState("");
  const [sorting, setSorting] = useState<SortingState>([]);
  const [columnSizing, setColumnSizing] = useState<ColumnSizingState>({});

  const numericCols = useMemo(
    () => columns.map((c, i) => detectNumeric(c.typeHint, rows, i)),
    [columns, rows]
  );

  const tableColumns = useMemo<ColumnDef<ResultRow>[]>(
    () =>
      columns.map((c, i) => ({
        id: `${i}:${c.name}`,
        accessorFn: (row) => row[i],
        header: c.name,
        meta: {
          typeHint: c.typeHint,
          numeric: numericCols[i],
        } satisfies ColumnMeta,
        cell: ({ getValue }) => renderCell(getValue(), setMenu),
        // Fallback `size` used only until the layout effect below
        // measures the container and computes real defaults. Picked
        // to be unobtrusive if the effect somehow doesn't fire.
        size: numericCols[i] ? 120 : 220,
        minSize: 80,
        maxSize: 1200,
        sortingFn: (a, b, colId) =>
          compareCells(a.getValue(colId), b.getValue(colId)),
      })),
    [columns, numericCols]
  );

  const table = useReactTable({
    data: rows,
    columns: tableColumns,
    state: { globalFilter, sorting, columnSizing },
    onGlobalFilterChange: setGlobalFilter,
    onSortingChange: setSorting,
    onColumnSizingChange: setColumnSizing,
    getCoreRowModel: getCoreRowModel(),
    getFilteredRowModel: getFilteredRowModel(),
    getSortedRowModel: getSortedRowModel(),
    columnResizeMode: "onChange",
    enableColumnResizing: true,
    globalFilterFn: (row, _id, filterValue) => {
      const q = String(filterValue).toLowerCase();
      if (!q) return true;
      return (row.original as ResultRow).some((cell) => {
        if (cell === null || cell === undefined) return false;
        return String(cell).toLowerCase().includes(q);
      });
    },
  });

  const filteredRows = table.getRowModel().rows;
  const totalWidth = table.getTotalSize();

  const parentRef = useRef<HTMLDivElement>(null);

  // Compute content-aware default widths whenever a new result set
  // arrives. We deliberately don't include `containerWidth` as a dep —
  // once the user has the table laid out, panel resizes shouldn't
  // wipe their manual column adjustments. Recompute on `columns`/
  // `rows` only, both of which change exactly when a new query
  // response lands. `useLayoutEffect` runs before paint, so the user
  // never sees the placeholder defaults flash.
  useLayoutEffect(() => {
    const el = parentRef.current;
    if (!el) return;
    const containerWidth = el.clientWidth;
    if (containerWidth < 100) return;

    const sizes = computeColumnSizes(
      columns,
      rows,
      numericCols,
      containerWidth
    );
    const next: ColumnSizingState = {};
    columns.forEach((c, i) => {
      next[`${i}:${c.name}`] = sizes[i];
    });
    setColumnSizing(next);
  }, [columns, rows, numericCols]);
  const virtualizer = useVirtualizer({
    count: filteredRows.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 28,
    overscan: 12,
  });

  const isFiltered = globalFilter.trim().length > 0;
  const headerGroups = table.getHeaderGroups();

  return (
    <div className="flex h-full w-full flex-col border-t bg-background">
      <Toolbar
        filter={globalFilter}
        onFilterChange={setGlobalFilter}
        filteredCount={filteredRows.length}
        totalCount={rows.length}
        isFiltered={isFiltered}
      />
      <div
        ref={parentRef}
        className="flex-1 overflow-auto"
        // Style needed so the sticky thead row pins inside the scroller.
      >
        <div
          role="table"
          className="relative font-mono text-sm"
          style={{ width: Math.max(totalWidth, 100), minWidth: "100%" }}
        >
          {headerGroups.map((hg) => (
            <div
              key={hg.id}
              role="row"
              className="sticky top-0 z-10 flex border-b bg-background"
              style={{ width: totalWidth }}
            >
              {hg.headers.map((h) => {
                const meta = h.column.columnDef.meta as ColumnMeta | undefined;
                const numeric = meta?.numeric ?? false;
                const sort = h.column.getIsSorted();
                return (
                  <div
                    key={h.id}
                    role="columnheader"
                    style={{ width: h.getSize() }}
                    className="relative flex shrink-0 items-center border-r last:border-r-0"
                  >
                    <button
                      type="button"
                      onClick={h.column.getToggleSortingHandler()}
                      className={cn(
                        "group flex h-full w-full items-center gap-1.5 px-3 py-2 text-left transition-colors hover:bg-accent",
                        numeric && "justify-end"
                      )}
                      title={
                        sort === "asc"
                          ? "Sorted ascending — click to sort descending"
                          : sort === "desc"
                            ? "Sorted descending — click to clear"
                            : "Click to sort"
                      }
                    >
                      <span className="truncate font-medium text-foreground">
                        {flexRender(h.column.columnDef.header, h.getContext())}
                      </span>
                      {meta?.typeHint && (
                        <span className="hidden text-[10px] uppercase tracking-wide text-muted-foreground/70 lg:inline">
                          {meta.typeHint}
                        </span>
                      )}
                      <SortIndicator
                        sort={sort}
                        className={numeric ? "" : "ml-auto"}
                      />
                    </button>
                    <div
                      onMouseDown={h.getResizeHandler()}
                      onTouchStart={h.getResizeHandler()}
                      onDoubleClick={() => h.column.resetSize()}
                      className={cn(
                        "absolute right-0 top-0 h-full w-1 cursor-col-resize select-none touch-none transition-colors",
                        h.column.getIsResizing()
                          ? "bg-ring"
                          : "hover:bg-ring/40"
                      )}
                      title="Drag to resize, double-click to reset"
                    />
                  </div>
                );
              })}
            </div>
          ))}
          <div
            role="rowgroup"
            className="relative"
            style={{ height: virtualizer.getTotalSize() }}
          >
            {virtualizer.getVirtualItems().map((vi) => {
              const row = filteredRows[vi.index];
              if (!row) return null;
              return (
                <div
                  key={row.id}
                  role="row"
                  className="absolute left-0 flex border-b transition-colors hover:bg-accent/40"
                  style={{
                    width: totalWidth,
                    height: vi.size,
                    transform: `translateY(${vi.start}px)`,
                  }}
                >
                  {row.getVisibleCells().map((cell) => {
                    const meta = cell.column.columnDef.meta as
                      | ColumnMeta
                      | undefined;
                    const value = cell.getValue();
                    return (
                      <div
                        key={cell.id}
                        role="cell"
                        style={{ width: cell.column.getSize() }}
                        className={cn(
                          "shrink-0 truncate border-r px-3 py-1.5 text-foreground/90 last:border-r-0",
                          meta?.numeric && "text-right tabular-nums"
                        )}
                        title={titleFor(value)}
                      >
                        {flexRender(
                          cell.column.columnDef.cell,
                          cell.getContext()
                        )}
                      </div>
                    );
                  })}
                </div>
              );
            })}
          </div>
          {isFiltered && filteredRows.length === 0 && (
            <div className="p-6 text-center text-sm text-muted-foreground">
              No rows match &ldquo;{globalFilter}&rdquo;
            </div>
          )}
        </div>
      </div>
      {menu && (
        <IriContextMenu
          x={menu.x}
          y={menu.y}
          iri={menu.iri}
          onClose={() => setMenu(null)}
        />
      )}
    </div>
  );
}

function Toolbar({
  filter,
  onFilterChange,
  filteredCount,
  totalCount,
  isFiltered,
}: {
  filter: string;
  onFilterChange: (v: string) => void;
  filteredCount: number;
  totalCount: number;
  isFiltered: boolean;
}) {
  return (
    <div className="flex items-center gap-2 border-b bg-background px-3 py-1.5">
      <div className="relative flex max-w-xs flex-1 items-center">
        <Search
          className="absolute left-2 size-3.5 text-muted-foreground/70"
          aria-hidden
        />
        <input
          type="text"
          value={filter}
          onChange={(e) => onFilterChange(e.target.value)}
          placeholder="Filter rows…"
          className="h-7 w-full rounded-md border bg-background pl-7 pr-7 text-xs text-foreground outline-none placeholder:text-muted-foreground focus:ring-1 focus:ring-ring"
        />
        {filter && (
          <button
            type="button"
            onClick={() => onFilterChange("")}
            className="absolute right-1.5 text-muted-foreground/70 transition-colors hover:text-foreground"
            aria-label="Clear filter"
          >
            <X size={12} />
          </button>
        )}
      </div>
      <span className="text-xs tabular-nums text-muted-foreground">
        {isFiltered
          ? `${filteredCount.toLocaleString()} of ${totalCount.toLocaleString()}`
          : `${totalCount.toLocaleString()} rows`}
      </span>
    </div>
  );
}

function SortIndicator({
  sort,
  className,
}: {
  sort: false | "asc" | "desc";
  className?: string;
}) {
  if (sort === "asc") {
    return <ArrowUp className={cn("size-3 text-foreground", className)} />;
  }
  if (sort === "desc") {
    return <ArrowDown className={cn("size-3 text-foreground", className)} />;
  }
  return (
    <ChevronsUpDown
      className={cn(
        "size-3 text-muted-foreground/30 opacity-0 transition-opacity group-hover:opacity-100",
        className
      )}
    />
  );
}

function compareCells(a: unknown, b: unknown): number {
  if (a === b) return 0;
  if (a === null || a === undefined) return 1;
  if (b === null || b === undefined) return -1;
  if (typeof a === "number" && typeof b === "number") return a - b;
  if (typeof a === "boolean" && typeof b === "boolean") {
    return a === b ? 0 : a ? 1 : -1;
  }
  return String(a).localeCompare(String(b), undefined, { numeric: true });
}

function titleFor(v: unknown): string {
  if (v === null || v === undefined) return "null";
  if (typeof v === "string") return v;
  if (typeof v === "number" || typeof v === "boolean") return String(v);
  try {
    return JSON.stringify(v);
  } catch {
    return String(v);
  }
}

type MenuSetter = (m: { x: number; y: number; iri: string } | null) => void;

function renderCell(v: unknown, setMenu: MenuSetter): React.ReactNode {
  if (v === null || v === undefined) {
    return <span className="italic text-muted-foreground/60">null</span>;
  }
  if (typeof v === "boolean") {
    return <span className="text-warning">{String(v)}</span>;
  }
  if (typeof v === "number") {
    return (
      <span className="tabular-nums text-foreground">
        {Number.isInteger(v) ? v.toLocaleString() : v}
      </span>
    );
  }
  if (typeof v === "string") {
    const looksLikeIri = /^[a-z][a-z0-9+.-]*:\/\//i.test(v);
    if (looksLikeIri) {
      return (
        <span
          className="cursor-context-menu font-medium text-foreground underline decoration-muted-foreground/40 underline-offset-2 hover:decoration-foreground"
          onContextMenu={(e) => {
            e.preventDefault();
            setMenu({ x: e.clientX, y: e.clientY, iri: v });
          }}
        >
          {v}
        </span>
      );
    }
    return <span className="text-foreground/90">{v}</span>;
  }
  return <span className="text-muted-foreground">{JSON.stringify(v)}</span>;
}
