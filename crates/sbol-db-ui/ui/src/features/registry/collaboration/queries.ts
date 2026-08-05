import { useQuery } from "@tanstack/react-query";

import {
  fetchCollaborators,
  fetchObjectActivity,
  fetchObjectReview,
  fetchReviews,
} from "./api";

export const collaborationKeys = {
  all: ["registry", "collaboration"] as const,
  collaborators: (iri: string) =>
    [...collaborationKeys.all, "collaborators", iri] as const,
  reviews: () => [...collaborationKeys.all, "reviews"] as const,
  review: (iri: string) => [...collaborationKeys.reviews(), iri] as const,
  activity: (iri: string) =>
    [...collaborationKeys.all, "activity", iri] as const,
};

export function useCollaborators(iri: string, enabled = true) {
  return useQuery({
    queryKey: collaborationKeys.collaborators(iri),
    queryFn: ({ signal }) => fetchCollaborators(iri, signal),
    enabled: enabled && iri.length > 0,
    staleTime: 15_000,
    retry: false,
  });
}

export function useReviews(enabled = true) {
  return useQuery({
    queryKey: collaborationKeys.reviews(),
    queryFn: ({ signal }) => fetchReviews(signal),
    enabled,
    staleTime: 15_000,
    retry: false,
  });
}

export function useObjectReview(iri: string, enabled = true) {
  return useQuery({
    queryKey: collaborationKeys.review(iri),
    queryFn: ({ signal }) => fetchObjectReview(iri, signal),
    enabled: enabled && iri.length > 0,
    staleTime: 15_000,
    retry: false,
  });
}

export function useObjectActivity(iri: string, enabled = true) {
  return useQuery({
    queryKey: collaborationKeys.activity(iri),
    queryFn: ({ signal }) => fetchObjectActivity(iri, signal),
    enabled: enabled && iri.length > 0,
    staleTime: 15_000,
    retry: false,
  });
}
