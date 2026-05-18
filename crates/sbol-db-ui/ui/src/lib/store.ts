/**
 * Client-side state for the lab bench.
 *
 * Persisted via Zustand's `persist` middleware into `localStorage`.
 * Stores recent query history (capped), saved queries, the last-used
 * dialect, and per-dialect editor buffers so switching tabs doesn't
 * lose the working text.
 */

import { create } from "zustand";
import { persist } from "zustand/middleware";

export type Dialect = "sql" | "sparql";

export interface HistoryEntry {
  id: string;
  dialect: Dialect;
  query: string;
  ranAt: number;
  elapsedMs: number;
  rowCount: number;
  ok: boolean;
  errorMessage?: string;
}

export interface SavedQuery {
  id: string;
  name: string;
  dialect: Dialect;
  query: string;
  updatedAt: number;
}

interface LabState {
  /** Last-opened dialect. Controls the default `/lab` redirect. */
  lastDialect: Dialect;
  /** Working text per dialect so switching tabs doesn't clobber input. */
  buffers: Record<Dialect, string>;
  /** Run history, newest first. Capped to keep localStorage bounded. */
  history: HistoryEntry[];
  /** Named saved queries. */
  saved: SavedQuery[];
  /** Recent sequence-search motifs, newest first. */
  recentSeqPatterns: string[];

  setDialect: (d: Dialect) => void;
  setBuffer: (d: Dialect, text: string) => void;
  pushHistory: (entry: Omit<HistoryEntry, "id">) => void;
  clearHistory: () => void;
  saveQuery: (
    q: Omit<SavedQuery, "id" | "updatedAt"> & { id?: string }
  ) => SavedQuery;
  deleteSaved: (id: string) => void;
  rememberSeqPattern: (pattern: string) => void;
}

const HISTORY_CAP = 50;
const SEQ_PATTERN_CAP = 20;

const DEFAULT_SPARQL = `PREFIX sbol: <http://sbols.org/v3#>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>

SELECT ?component ?name WHERE {
  ?component a sbol:Component .
  OPTIONAL { ?component sbol:name ?name }
}
LIMIT 25
`;

const DEFAULT_SQL = `-- Top SBOL classes by row count.
SELECT sbol_class, count(*) AS objects
FROM sbol_objects
GROUP BY sbol_class
ORDER BY objects DESC
LIMIT 25;
`;

function uuid(): string {
  // crypto.randomUUID isn't available everywhere; cheap fallback for
  // localStorage IDs is fine — we don't need cryptographic strength.
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID();
  }
  return Math.random().toString(36).slice(2) + Date.now().toString(36);
}

export const useLabStore = create<LabState>()(
  persist(
    (set) => ({
      lastDialect: "sparql",
      buffers: { sql: DEFAULT_SQL, sparql: DEFAULT_SPARQL },
      history: [],
      saved: [],
      recentSeqPatterns: [],

      setDialect: (d) => set({ lastDialect: d }),
      setBuffer: (d, text) =>
        set((s) => ({ buffers: { ...s.buffers, [d]: text } })),
      pushHistory: (entry) =>
        set((s) => ({
          history: [{ id: uuid(), ...entry }, ...s.history].slice(
            0,
            HISTORY_CAP
          ),
        })),
      clearHistory: () => set({ history: [] }),
      saveQuery: (q) => {
        const now = Date.now();
        const id = q.id ?? uuid();
        const next: SavedQuery = {
          id,
          name: q.name,
          dialect: q.dialect,
          query: q.query,
          updatedAt: now,
        };
        set((s) => {
          const filtered = s.saved.filter((x) => x.id !== id);
          return { saved: [next, ...filtered] };
        });
        return next;
      },
      deleteSaved: (id) =>
        set((s) => ({ saved: s.saved.filter((x) => x.id !== id) })),
      rememberSeqPattern: (pattern) => {
        const trimmed = pattern.trim();
        if (!trimmed) return;
        set((s) => ({
          recentSeqPatterns: [
            trimmed,
            ...s.recentSeqPatterns.filter((p) => p !== trimmed),
          ].slice(0, SEQ_PATTERN_CAP),
        }));
      },
    }),
    {
      name: "sbol-lab-state-v1",
      version: 1,
    }
  )
);
