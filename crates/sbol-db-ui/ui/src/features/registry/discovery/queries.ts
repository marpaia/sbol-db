import { useQuery } from "@tanstack/react-query";

import {
  fetchDiscoveryFacets,
  fetchSearchStrategies,
  searchPortal,
  searchPortalSequences,
  searchPortalStructured,
  type PortalSearchQuery,
  type StructuredSearchRequest,
} from "./api";

export const discoveryKeys = {
  all: ["registry", "discovery"] as const,
  searches: () => [...discoveryKeys.all, "search"] as const,
  search: (query: PortalSearchQuery) =>
    [...discoveryKeys.searches(), query] as const,
  facets: () => [...discoveryKeys.all, "facets"] as const,
  strategies: () => [...discoveryKeys.all, "strategies"] as const,
  structured: (request: StructuredSearchRequest) =>
    [...discoveryKeys.searches(), "structured", request] as const,
  sequence: (query: { q: string; mode: string; limit: number }) =>
    [
      ...discoveryKeys.searches(),
      "sequence",
      query.q,
      query.mode,
      query.limit,
    ] as const,
};

export function usePortalSearch(query: PortalSearchQuery, enabled = true) {
  return useQuery({
    queryKey: discoveryKeys.search(query),
    queryFn: ({ signal }) => searchPortal(query, signal),
    enabled,
    staleTime: 30_000,
  });
}

export function useDiscoveryFacets(enabled = true) {
  return useQuery({
    queryKey: discoveryKeys.facets(),
    queryFn: ({ signal }) => fetchDiscoveryFacets(signal),
    enabled,
    staleTime: 60_000,
  });
}

export function useSearchStrategies() {
  return useQuery({
    queryKey: discoveryKeys.strategies(),
    queryFn: ({ signal }) => fetchSearchStrategies(signal),
    staleTime: 60_000,
    retry: false,
  });
}

export function useStructuredSearch(
  request: StructuredSearchRequest,
  enabled = true
) {
  return useQuery({
    queryKey: discoveryKeys.structured(request),
    queryFn: ({ signal }) => searchPortalStructured(request, signal),
    enabled,
    staleTime: 30_000,
    retry: false,
  });
}

export function useSequenceSearch(query: {
  q: string;
  mode: "global" | "exact";
  limit: number;
}) {
  return useQuery({
    queryKey: discoveryKeys.sequence(query),
    queryFn: ({ signal }) => searchPortalSequences(query, signal),
    enabled: query.q.length > 0,
    staleTime: 30_000,
    retry: false,
  });
}
