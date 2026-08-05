/** TanStack Query wrappers around the lab's schema endpoints. */

import { useQuery } from "@tanstack/react-query";
import { fetchSparqlSchema, fetchSqlSchema } from "./api";

export const schemaKeys = {
  all: ["admin", "schema"] as const,
  sql: () => [...schemaKeys.all, "sql"] as const,
  sparql: () => [...schemaKeys.all, "sparql"] as const,
};

const FRESH_MS = 60_000;

export function useSqlSchema() {
  return useQuery({
    queryKey: schemaKeys.sql(),
    queryFn: ({ signal }) => fetchSqlSchema(signal),
    staleTime: FRESH_MS,
  });
}

export function useSparqlSchema() {
  return useQuery({
    queryKey: schemaKeys.sparql(),
    queryFn: ({ signal }) => fetchSparqlSchema(signal),
    staleTime: FRESH_MS,
  });
}
