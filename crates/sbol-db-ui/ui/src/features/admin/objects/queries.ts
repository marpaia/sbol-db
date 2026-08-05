/** TanStack Query wrappers for the typed object endpoints. */

import { useMutation, useQuery } from "@tanstack/react-query";

import {
  exportObjectRdf,
  getObjectByIri,
  listObjects,
  lookupObjects,
  type ListObjectsQuery,
  type LookupObjectsResponse,
} from "./api";
import type { SerializationFormat } from "@/features/admin/imports/api";

const FRESH_MS = 30_000;

export const objectKeys = {
  all: ["admin", "objects"] as const,
  lists: () => [...objectKeys.all, "list"] as const,
  list: (query: ListObjectsQuery) =>
    [
      ...objectKeys.lists(),
      query.sbol_class ?? "",
      query.role ?? "",
      query.graph_id ?? "",
      query.after ?? "",
      query.limit ?? null,
    ] as const,
  detail: (iri: string) => [...objectKeys.all, "detail", iri] as const,
  byGraph: (graphId: string, after: string | null | undefined) =>
    [...objectKeys.all, "by-graph", graphId, after ?? ""] as const,
  rdf: (id: string, format: SerializationFormat) =>
    [...objectKeys.all, "rdf", id, format] as const,
};

export function useObjectsList(query: ListObjectsQuery) {
  return useQuery({
    queryKey: objectKeys.list(query),
    queryFn: ({ signal }) => listObjects(query, signal),
    staleTime: FRESH_MS,
    placeholderData: (prev) => prev,
  });
}

export function useObjectByIri(iri: string) {
  return useQuery({
    queryKey: objectKeys.detail(iri),
    queryFn: ({ signal }) => getObjectByIri(iri, signal),
    enabled: iri.length > 0,
    staleTime: FRESH_MS,
    retry: false,
  });
}

export function useObjectLookup() {
  return useMutation<LookupObjectsResponse, Error, string[]>({
    mutationFn: (iris) => lookupObjects(iris),
  });
}

export function useObjectRdf(
  id: string,
  format: SerializationFormat,
  enabled = false
) {
  return useQuery({
    queryKey: objectKeys.rdf(id, format),
    queryFn: ({ signal }) => exportObjectRdf(id, format, signal),
    enabled: enabled && id.length > 0,
    staleTime: FRESH_MS,
    retry: false,
  });
}
