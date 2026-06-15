/** TanStack Query wrappers for the typed object endpoints. */

import { useMutation, useQuery } from "@tanstack/react-query";

import {
  exportObjectRdf,
  fetchNeighborhood,
  fetchNeighborhoodRdf,
  getObjectByIri,
  listObjects,
  lookupObjects,
  type ListObjectsQuery,
  type LookupObjectsResponse,
  type NeighborhoodQuery,
  type SerializationFormat,
} from "@/lib/api";

const FRESH_MS = 30_000;

export function useObjectsList(query: ListObjectsQuery) {
  return useQuery({
    queryKey: [
      "lab",
      "objects",
      "list",
      query.sbol_class ?? "",
      query.role ?? "",
      query.graph_id ?? "",
      query.after ?? "",
      query.limit ?? null,
    ],
    queryFn: ({ signal }) => listObjects(query, signal),
    staleTime: FRESH_MS,
    placeholderData: (prev) => prev,
  });
}

export function useObjectByIri(iri: string) {
  return useQuery({
    queryKey: ["lab", "objects", "by-iri", iri],
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

export function useNeighborhood(query: NeighborhoodQuery, enabled = true) {
  return useQuery({
    queryKey: [
      "lab",
      "neighborhood",
      query.iri,
      query.depth ?? null,
      query.direction ?? "",
      (query.predicates ?? []).join(","),
      query.max_nodes ?? null,
      query.literals ?? null,
    ],
    queryFn: ({ signal }) => fetchNeighborhood(query, signal),
    enabled: enabled && query.iri.length > 0,
    staleTime: FRESH_MS,
    retry: false,
  });
}

export function useNeighborhoodRdf(
  query: NeighborhoodQuery,
  format: SerializationFormat,
  enabled = true
) {
  return useQuery({
    queryKey: [
      "lab",
      "neighborhood-rdf",
      query.iri,
      query.depth ?? null,
      query.direction ?? "",
      (query.predicates ?? []).join(","),
      query.max_nodes ?? null,
      query.literals ?? null,
      format,
    ],
    queryFn: ({ signal }) => fetchNeighborhoodRdf(query, format, signal),
    enabled: enabled && query.iri.length > 0,
    staleTime: FRESH_MS,
    retry: false,
  });
}

export function useObjectRdf(
  id: string,
  format: SerializationFormat,
  enabled = false
) {
  return useQuery({
    queryKey: ["lab", "objects", "rdf", id, format],
    queryFn: ({ signal }) => exportObjectRdf(id, format, signal),
    enabled: enabled && id.length > 0,
    staleTime: FRESH_MS,
    retry: false,
  });
}
