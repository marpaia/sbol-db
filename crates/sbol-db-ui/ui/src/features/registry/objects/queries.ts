import { useQuery } from "@tanstack/react-query";

import {
  fetchPortalObject,
  fetchPortalObjectDetails,
  fetchSimilarObjects,
} from "./api";

export const registryObjectKeys = {
  all: ["registry", "objects"] as const,
  detail: (iri: string) => [...registryObjectKeys.all, "detail", iri] as const,
  normalized: (iri: string) =>
    [...registryObjectKeys.detail(iri), "normalized"] as const,
  similar: (iri: string) =>
    [...registryObjectKeys.detail(iri), "similar"] as const,
};

export function useSimilarObjects(iri: string) {
  return useQuery({
    queryKey: registryObjectKeys.similar(iri),
    queryFn: ({ signal }) => fetchSimilarObjects(iri, signal),
    enabled: iri.length > 0,
    staleTime: 30_000,
    retry: false,
  });
}

export function usePortalObject(iri: string) {
  return useQuery({
    queryKey: registryObjectKeys.detail(iri),
    queryFn: ({ signal }) => fetchPortalObject(iri, signal),
    enabled: iri.length > 0,
    staleTime: 30_000,
    retry: false,
  });
}

export function usePortalObjectDetails(iri: string) {
  return useQuery({
    queryKey: registryObjectKeys.normalized(iri),
    queryFn: ({ signal }) => fetchPortalObjectDetails(iri, signal),
    enabled: iri.length > 0,
    staleTime: 30_000,
    retry: false,
  });
}
