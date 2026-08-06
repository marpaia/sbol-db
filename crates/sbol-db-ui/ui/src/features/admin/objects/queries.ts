/** TanStack Query wrappers for the universal RDF resource catalog. */

import { useMutation, useQuery } from "@tanstack/react-query";

import {
  getObjectByIri,
  listObjects,
  lookupObjects,
  type ListObjectsQuery,
  type LookupObjectsResponse,
} from "./api";

const FRESH_MS = 30_000;

export const objectKeys = {
  all: ["admin", "objects"] as const,
  lists: () => [...objectKeys.all, "list"] as const,
  list: (query: ListObjectsQuery) =>
    [
      ...objectKeys.lists(),
      query.class ?? "",
      query.role ?? "",
      query.graph ?? "",
      query.q ?? "",
      query.after ?? "",
      query.limit ?? null,
    ] as const,
  detail: (iri: string) => [...objectKeys.all, "detail", iri] as const,
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
