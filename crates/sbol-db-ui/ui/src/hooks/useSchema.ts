/** TanStack Query wrappers around the lab's schema endpoints. */

import { useQuery } from "@tanstack/react-query";
import { fetchSparqlSchema, fetchSqlSchema } from "@/lib/api";

const FRESH_MS = 60_000;

export function useSqlSchema() {
  return useQuery({
    queryKey: ["lab", "schema", "sql"],
    queryFn: ({ signal }) => fetchSqlSchema(signal),
    staleTime: FRESH_MS,
  });
}

export function useSparqlSchema() {
  return useQuery({
    queryKey: ["lab", "schema", "sparql"],
    queryFn: ({ signal }) => fetchSparqlSchema(signal),
    staleTime: FRESH_MS,
  });
}
