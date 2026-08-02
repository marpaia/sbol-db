import { useQuery } from "@tanstack/react-query";

import {
  fetchAccount,
  fetchCollaborators,
  fetchDiscoveryFacets,
  fetchInstance,
  fetchObjectActivity,
  fetchObjectReview,
  fetchPortalObjectDetails,
  fetchPortalObject,
  fetchSession,
  fetchSharedObjects,
  fetchSimilarObjects,
  fetchReviews,
  fetchSearchStrategies,
  type PortalSearchQuery,
  type StructuredSearchRequest,
  searchPortalSequences,
  searchPortalStructured,
  searchPortal,
} from "./api";

export const portalKeys = {
  instance: ["portal", "instance"] as const,
  session: ["portal", "session"] as const,
  search: (query: PortalSearchQuery) => ["portal", "search", query] as const,
  facets: ["portal", "search", "facets"] as const,
  strategies: ["portal", "search", "strategies"] as const,
  structuredSearch: (request: StructuredSearchRequest) =>
    ["portal", "search", "structured", request] as const,
  sequenceSearch: (q: string, mode: string, limit: number) =>
    ["portal", "sequence-search", q, mode, limit] as const,
  similar: (iri: string) => ["portal", "similar", iri] as const,
  object: (iri: string) => ["portal", "object", iri] as const,
  objectDetails: (iri: string) => ["portal", "object-details", iri] as const,
  account: ["portal", "account"] as const,
  shared: ["portal", "account", "shared"] as const,
  sharedPage: (offset: number, limit: number) =>
    ["portal", "account", "shared", offset, limit] as const,
  collaborators: (iri: string) => ["portal", "collaborators", iri] as const,
  reviews: ["portal", "reviews"] as const,
  objectReview: (iri: string) => ["portal", "reviews", iri] as const,
  objectActivity: (iri: string) => ["portal", "activity", iri] as const,
};

export function useInstance() {
  return useQuery({
    queryKey: portalKeys.instance,
    queryFn: ({ signal }) => fetchInstance(signal),
    staleTime: 60_000,
  });
}

export function useSession() {
  return useQuery({
    queryKey: portalKeys.session,
    queryFn: ({ signal }) => fetchSession(signal),
    staleTime: 15_000,
    retry: false,
  });
}

export function useAccount(enabled = true) {
  return useQuery({
    queryKey: portalKeys.account,
    queryFn: ({ signal }) => fetchAccount(signal),
    enabled,
    staleTime: 15_000,
    retry: false,
  });
}

export function useSharedObjects(offset = 0, limit = 24, enabled = true) {
  return useQuery({
    queryKey: portalKeys.sharedPage(offset, limit),
    queryFn: ({ signal }) => fetchSharedObjects({ offset, limit }, signal),
    enabled,
    staleTime: 15_000,
    retry: false,
  });
}

export function useCollaborators(iri: string, enabled = true) {
  return useQuery({
    queryKey: portalKeys.collaborators(iri),
    queryFn: ({ signal }) => fetchCollaborators(iri, signal),
    enabled: enabled && iri.length > 0,
    staleTime: 15_000,
    retry: false,
  });
}

export function useReviews(enabled = true) {
  return useQuery({
    queryKey: portalKeys.reviews,
    queryFn: ({ signal }) => fetchReviews(signal),
    enabled,
    staleTime: 15_000,
    retry: false,
  });
}

export function useObjectReview(iri: string, enabled = true) {
  return useQuery({
    queryKey: portalKeys.objectReview(iri),
    queryFn: ({ signal }) => fetchObjectReview(iri, signal),
    enabled: enabled && iri.length > 0,
    staleTime: 15_000,
    retry: false,
  });
}

export function useObjectActivity(iri: string, enabled = true) {
  return useQuery({
    queryKey: portalKeys.objectActivity(iri),
    queryFn: ({ signal }) => fetchObjectActivity(iri, signal),
    enabled: enabled && iri.length > 0,
    staleTime: 15_000,
    retry: false,
  });
}

export function usePortalSearch(query: PortalSearchQuery, enabled = true) {
  return useQuery({
    queryKey: portalKeys.search(query),
    queryFn: ({ signal }) => searchPortal(query, signal),
    enabled,
    staleTime: 30_000,
  });
}

export function useDiscoveryFacets(enabled = true) {
  return useQuery({
    queryKey: portalKeys.facets,
    queryFn: ({ signal }) => fetchDiscoveryFacets(signal),
    enabled,
    staleTime: 60_000,
  });
}

export function useSearchStrategies() {
  return useQuery({
    queryKey: portalKeys.strategies,
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
    queryKey: portalKeys.structuredSearch(request),
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
    queryKey: portalKeys.sequenceSearch(query.q, query.mode, query.limit),
    queryFn: ({ signal }) => searchPortalSequences(query, signal),
    enabled: query.q.length > 0,
    staleTime: 30_000,
    retry: false,
  });
}

export function useSimilarObjects(iri: string) {
  return useQuery({
    queryKey: portalKeys.similar(iri),
    queryFn: ({ signal }) => fetchSimilarObjects(iri, signal),
    enabled: iri.length > 0,
    staleTime: 30_000,
    retry: false,
  });
}

export function usePortalObject(iri: string) {
  return useQuery({
    queryKey: portalKeys.object(iri),
    queryFn: ({ signal }) => fetchPortalObject(iri, signal),
    enabled: iri.length > 0,
    staleTime: 30_000,
    retry: false,
  });
}

export function usePortalObjectDetails(iri: string) {
  return useQuery({
    queryKey: portalKeys.objectDetails(iri),
    queryFn: ({ signal }) => fetchPortalObjectDetails(iri, signal),
    enabled: iri.length > 0,
    staleTime: 30_000,
    retry: false,
  });
}
