import { useQuery } from "@tanstack/react-query";

import {
  fetchNeighborhood,
  fetchNeighborhoodRdf,
  type NeighborhoodQuery,
  type SerializationFormat,
} from "./api";

const FRESH_MS = 30_000;

function neighborhoodParameters(query: NeighborhoodQuery) {
  return [
    query.iri,
    query.depth ?? null,
    query.direction ?? "",
    (query.predicates ?? []).join(","),
    query.max_nodes ?? null,
    query.literals ?? null,
  ] as const;
}

export const neighborhoodKeys = {
  all: ["admin", "neighborhood"] as const,
  table: (query: NeighborhoodQuery) =>
    [
      ...neighborhoodKeys.all,
      "table",
      ...neighborhoodParameters(query),
    ] as const,
  rdf: (query: NeighborhoodQuery, format: SerializationFormat) =>
    [
      ...neighborhoodKeys.all,
      "rdf",
      ...neighborhoodParameters(query),
      format,
    ] as const,
};

export function useNeighborhood(query: NeighborhoodQuery, enabled = true) {
  return useQuery({
    queryKey: neighborhoodKeys.table(query),
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
    queryKey: neighborhoodKeys.rdf(query, format),
    queryFn: ({ signal }) => fetchNeighborhoodRdf(query, format, signal),
    enabled: enabled && query.iri.length > 0,
    staleTime: FRESH_MS,
    retry: false,
  });
}
