/**
 * Postgres SQL highlighting via `monaco-sql-languages` (pgsql flavour).
 *
 * The package contributes the language registration as a side effect
 * when imported. We re-export a stable identifier for the rest of the
 * app and provide a one-shot register hook that's idempotent.
 *
 * Future PRs will plug an async marker provider into this same module
 * for libpg_query-driven validation; until then, this is pure
 * syntax-only highlighting + simple keyword completion.
 */

import "monaco-sql-languages/esm/languages/pgsql/pgsql.contribution";

import type * as MonacoNS from "monaco-editor";

export const SQL_LANGUAGE_ID = "pgsql";

let registered = false;

export function registerSql(_monaco: typeof MonacoNS): void {
  if (registered) return;
  registered = true;
  // The contribution import above does the heavy lifting; this hook
  // exists so future PRs can register validators, hover providers,
  // etc. on a single deterministic call.
}
