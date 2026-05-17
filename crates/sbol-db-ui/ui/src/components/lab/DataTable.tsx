/**
 * Typed table primitive for the lab's read-only views (observability,
 * Postgres maintenance, etc).
 *
 * Shares the visual approach of `ResultsTable` — flex rows + fixed
 * pixel widths per column, sticky header pinned inside the scroll
 * container — so columns stay aligned with the header at every row
 * height. Unlike `ResultsTable` it accepts typed rows and a custom
 * `cell` renderer per column (badges, formatted bytes/durations,
 * etc.) and supports a clickable row callback for drill-downs.
 *
 * Scope: bounded row counts (~hundreds). No virtualization — these
 * are dashboard views, not arbitrary query result sets. For arbitrary
 * SQL/SPARQL results use `ResultsTable` instead.
 */

import { useMemo, useState } from "react";
import { ArrowDown, ArrowUp, ChevronRight, ChevronsUpDown, Search, X } from "lucide-react";

import { cn } from "@/lib/utils";

export interface DataTableColumn<T> {
  /** Stable identifier; also the sort key. */
  id: string;
  /** Column heading shown in the sticky header. */
  header: string;
  /** Pixel width. Cells truncate with ellipsis past this width. */
  width: number;
  /** Right-align (default left). Useful for numeric columns. */
  align?: "left" | "right";
  /** Render the cell for a row. */
  cell: (row: T) => React.ReactNode;
  /**
   * Value used for sorting. Returning a number sorts numerically;
   * returning a string sorts lexicographically. Omit to make the
   * column non-sortable.
   */
  sortValue?: (row: T) => string | number | null | undefined;
  /**
   * Optional accessor for the global filter input. Returning a string
   * makes the row matchable by that text. Returning undefined excludes
   * the column from filter matching.
   */
  filterValue?: (row: T) => string | undefined;
}

export interface DataTableProps<T> {
  columns: DataTableColumn<T>[];
  rows: T[];
  /** Stable key per row. */
  rowKey: (row: T) => string;
  /** Show a filter input above the table. */
  filterable?: boolean;
  /** Make rows clickable. Adds a hover indicator and chevron. */
  onRowClick?: (row: T) => void;
  /** Rendered when `rows` is empty after filtering. */
  emptyMessage?: React.ReactNode;
  /** Cap the body height. Defaults to no cap (parent scrolls). */
  maxHeightClass?: string;
  /** Initial sort to apply on mount. */
  defaultSort?: { id: string; direction: "asc" | "desc" };
}

type Direction = "asc" | "desc";
interface SortState {
  id: string;
  direction: Direction;
}

export function DataTable<T>({
  columns,
  rows,
  rowKey,
  filterable,
  onRowClick,
  emptyMessage,
  maxHeightClass,
  defaultSort,
}: DataTableProps<T>) {
  const [sort, setSort] = useState<SortState | null>(defaultSort ?? null);
  const [filter, setFilter] = useState("");

  const filtered = useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (!q) return rows;
    return rows.filter((row) => {
      for (const col of columns) {
        if (!col.filterValue) continue;
        const v = col.filterValue(row);
        if (v && v.toLowerCase().includes(q)) return true;
      }
      return false;
    });
  }, [rows, columns, filter]);

  const sorted = useMemo(() => {
    if (!sort) return filtered;
    const col = columns.find((c) => c.id === sort.id);
    if (!col?.sortValue) return filtered;
    const accessor = col.sortValue;
    const mul = sort.direction === "asc" ? 1 : -1;
    return [...filtered].sort((a, b) => mul * compare(accessor(a), accessor(b)));
  }, [filtered, sort, columns]);

  const totalWidth = useMemo(
    () => columns.reduce((acc, c) => acc + c.width, 0) + (onRowClick ? 28 : 0),
    [columns, onRowClick]
  );

  // Right-aligned columns stay at their declared width (numbers, sizes,
  // durations — those should not stretch). Everything else grows
  // proportionally to its declared width so the table fills its
  // container instead of trailing dead space on the right.
  const hasGrowable = columns.some((c) => c.align !== "right");
  const colFlex = (c: DataTableColumn<T>): React.CSSProperties => {
    const grow = c.align === "right" && hasGrowable ? 0 : c.width;
    return { flex: `${grow} 0 ${c.width}px`, minWidth: c.width };
  };

  const onHeaderClick = (col: DataTableColumn<T>) => {
    if (!col.sortValue) return;
    setSort((prev) => {
      if (!prev || prev.id !== col.id) return { id: col.id, direction: "asc" };
      if (prev.direction === "asc") return { id: col.id, direction: "desc" };
      return null;
    });
  };

  return (
    <div className="flex w-full flex-col">
      {filterable && (
        <Toolbar
          filter={filter}
          onFilterChange={setFilter}
          filteredCount={sorted.length}
          totalCount={rows.length}
        />
      )}
      <div className={cn("w-full overflow-x-auto", maxHeightClass && "overflow-y-auto", maxHeightClass)}>
        <div role="table" style={{ minWidth: totalWidth }} className="text-xs">
          <div
            role="row"
            className="sticky top-0 z-10 flex border-b bg-card"
          >
            {columns.map((c) => {
              const isSorted = sort?.id === c.id;
              const sortable = !!c.sortValue;
              return (
                <button
                  type="button"
                  key={c.id}
                  role="columnheader"
                  disabled={!sortable}
                  onClick={() => onHeaderClick(c)}
                  className={cn(
                    "group flex items-center gap-1.5 border-r border-border/60 px-3 py-2 text-[10px] font-medium uppercase tracking-wider text-muted-foreground last:border-r-0",
                    sortable && "hover:bg-accent/40",
                    !sortable && "cursor-default",
                    c.align === "right" ? "justify-end" : "justify-start"
                  )}
                  style={colFlex(c)}
                  title={sortable ? "Click to sort" : undefined}
                >
                  <span className="truncate">{c.header}</span>
                  {sortable && (
                    <SortIndicator
                      direction={isSorted ? sort!.direction : null}
                      className={c.align === "right" ? "" : "ml-auto"}
                    />
                  )}
                </button>
              );
            })}
            {onRowClick && (
              <div
                className="shrink-0"
                style={{ flex: "0 0 28px", minWidth: 28 }}
                aria-hidden
              />
            )}
          </div>

          {sorted.length === 0 ? (
            <div className="px-4 py-6 text-sm text-muted-foreground">
              {emptyMessage ?? "No rows."}
            </div>
          ) : (
            <div role="rowgroup">
              {sorted.map((row) => {
                const clickable = !!onRowClick;
                return (
                  <div
                    key={rowKey(row)}
                    role="row"
                    onClick={clickable ? () => onRowClick!(row) : undefined}
                    tabIndex={clickable ? 0 : undefined}
                    onKeyDown={
                      clickable
                        ? (e) => {
                            if (e.key === "Enter" || e.key === " ") {
                              e.preventDefault();
                              onRowClick!(row);
                            }
                          }
                        : undefined
                    }
                    className={cn(
                      "group flex border-b border-border/60 transition-colors",
                      clickable &&
                        "cursor-pointer hover:bg-accent/40 focus-visible:bg-accent/40 focus-visible:outline-none"
                    )}
                  >
                    {columns.map((c) => (
                      <div
                        role="cell"
                        key={c.id}
                        className={cn(
                          "flex min-h-[2rem] items-center gap-1.5 border-r border-border/60 px-3 py-1.5 text-foreground/90 last:border-r-0",
                          c.align === "right" && "justify-end text-right tabular-nums"
                        )}
                        style={colFlex(c)}
                      >
                        <div
                          className={cn(
                            "min-w-0 truncate",
                            c.align === "right" && "text-right"
                          )}
                        >
                          {c.cell(row)}
                        </div>
                      </div>
                    ))}
                    {clickable && (
                      <div
                        className="flex items-center justify-center text-muted-foreground/40 transition-colors group-hover:text-foreground"
                        style={{ flex: "0 0 28px", minWidth: 28 }}
                      >
                        <ChevronRight size={12} />
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function Toolbar({
  filter,
  onFilterChange,
  filteredCount,
  totalCount,
}: {
  filter: string;
  onFilterChange: (v: string) => void;
  filteredCount: number;
  totalCount: number;
}) {
  const filtered = filter.trim().length > 0;
  return (
    <div className="flex items-center gap-2 border-b bg-card px-3 py-1.5">
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
        {filtered
          ? `${filteredCount.toLocaleString()} of ${totalCount.toLocaleString()}`
          : `${totalCount.toLocaleString()} rows`}
      </span>
    </div>
  );
}

function SortIndicator({
  direction,
  className,
}: {
  direction: Direction | null;
  className?: string;
}) {
  if (direction === "asc") {
    return <ArrowUp className={cn("size-3 text-foreground", className)} />;
  }
  if (direction === "desc") {
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

function compare(
  a: string | number | null | undefined,
  b: string | number | null | undefined
): number {
  if (a === b) return 0;
  if (a === null || a === undefined) return 1;
  if (b === null || b === undefined) return -1;
  if (typeof a === "number" && typeof b === "number") return a - b;
  return String(a).localeCompare(String(b), undefined, { numeric: true });
}
