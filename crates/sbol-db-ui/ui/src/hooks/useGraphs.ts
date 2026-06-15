/** TanStack Query wrappers for the graph-native endpoints. */

import { useQuery } from "@tanstack/react-query";

import { getGraph, listGraphs, type GraphsListQuery } from "@/lib/api";

const FRESH_MS = 30_000;

export function useGraphs(query: GraphsListQuery = {}) {
  return useQuery({
    queryKey: [
      "lab",
      "graphs",
      query.limit ?? null,
      query.offset ?? 0,
      query.kind ?? null,
    ],
    queryFn: ({ signal }) => listGraphs(query, signal),
    staleTime: FRESH_MS,
    placeholderData: (prev) => prev,
  });
}

export function useGraph(id: string) {
  return useQuery({
    queryKey: ["lab", "graphs", "detail", id],
    queryFn: ({ signal }) => getGraph(id, signal),
    enabled: id.length > 0,
    staleTime: FRESH_MS,
    retry: false,
  });
}
