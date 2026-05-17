/**
 * Right-rail panel: save the current buffer with a name, list past
 * saved queries, click to load, delete to remove. Backed by the
 * Zustand store's `saved` array (persisted to localStorage).
 */

import { useState } from "react";
import { Star, Trash2 } from "lucide-react";

import { type Dialect, useLabStore } from "@/lib/store";

export interface SavedQueriesListProps {
  dialect: Dialect;
  currentQuery: string;
  onLoad: (query: string) => void;
}

export function SavedQueriesList({
  dialect,
  currentQuery,
  onLoad,
}: SavedQueriesListProps) {
  const saved = useLabStore((s) =>
    s.saved.filter((q) => q.dialect === dialect)
  );
  const saveQuery = useLabStore((s) => s.saveQuery);
  const deleteSaved = useLabStore((s) => s.deleteSaved);
  const [name, setName] = useState("");

  return (
    <div className="flex h-full w-full flex-col border-l bg-background">
      <div className="flex items-center gap-2 border-b px-3 py-2 text-[11px] uppercase tracking-wider text-muted-foreground">
        <Star size={12} /> Saved
      </div>
      <form
        className="flex gap-2 border-b px-3 py-2"
        onSubmit={(e) => {
          e.preventDefault();
          if (!name.trim() || !currentQuery.trim()) return;
          saveQuery({ name: name.trim(), dialect, query: currentQuery });
          setName("");
        }}
      >
        <input
          type="text"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="Name this query…"
          className="flex-1 rounded border bg-background px-2 py-1 text-xs text-foreground outline-none placeholder:text-muted-foreground/60 focus:ring-1 focus:ring-ring"
        />
        <button
          type="submit"
          disabled={!name.trim()}
          className="rounded bg-primary px-2 py-1 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90 disabled:bg-muted disabled:text-muted-foreground"
        >
          Save
        </button>
      </form>
      <ul className="flex-1 overflow-y-auto">
        {saved.length === 0 && (
          <li className="px-3 py-2 text-xs text-muted-foreground">
            No saved queries yet.
          </li>
        )}
        {saved.map((q) => (
          <li key={q.id} className="group flex items-stretch border-b">
            <button
              type="button"
              onClick={() => onLoad(q.query)}
              className="flex-1 px-3 py-1.5 text-left transition-colors hover:bg-accent"
              title="Click to load"
            >
              <div className="truncate text-xs text-foreground">{q.name}</div>
              <div className="truncate font-mono text-[10px] text-muted-foreground">
                {q.query.split("\n").find((l) => l.trim()) ?? "(empty)"}
              </div>
            </button>
            <button
              type="button"
              onClick={() => deleteSaved(q.id)}
              className="px-2 text-muted-foreground/60 opacity-0 transition-opacity hover:text-destructive group-hover:opacity-100"
              aria-label="Delete"
              title="Delete"
            >
              <Trash2 size={14} />
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}
