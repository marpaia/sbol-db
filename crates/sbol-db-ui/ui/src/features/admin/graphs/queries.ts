/** TanStack Query wrappers for the graph-native endpoints. */

import { useQuery } from "@tanstack/react-query";

import { getGraph, listGraphs, type GraphsListQuery } from "./api";

export const graphKeys = {
  all: ["admin", "graphs"] as const,
  lists: () => [...graphKeys.all, "list"] as const,
  list: (query: GraphsListQuery) =>
    [
      ...graphKeys.lists(),
      query.limit ?? null,
      query.after ?? "",
      query.q ?? "",
    ] as const,
  detail: (id: string) => [...graphKeys.all, "detail", id] as const,
  triples: (id: string, after?: string) =>
    [...graphKeys.detail(id), "triples", after ?? ""] as const,
};

const FRESH_MS = 30_000;

export function useGraphs(query: GraphsListQuery = {}) {
  return useQuery({
    queryKey: graphKeys.list(query),
    queryFn: ({ signal }) => listGraphs(query, signal),
    staleTime: FRESH_MS,
    placeholderData: (prev) => prev,
  });
}

export function useGraph(id: string) {
  return useQuery({
    queryKey: graphKeys.detail(id),
    queryFn: ({ signal }) => getGraph(id, signal),
    enabled: id.length > 0,
    staleTime: FRESH_MS,
    retry: false,
  });
}
