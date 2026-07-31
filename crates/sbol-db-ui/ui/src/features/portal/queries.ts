import { useQuery } from "@tanstack/react-query";

import {
  fetchInstance,
  fetchPortalObject,
  fetchSession,
  searchPortal,
} from "./api";

export const portalKeys = {
  instance: ["portal", "instance"] as const,
  session: ["portal", "session"] as const,
  search: (q: string, type: string, offset: number, limit: number) =>
    ["portal", "search", q, type, offset, limit] as const,
  object: (iri: string) => ["portal", "object", iri] as const,
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

export function usePortalSearch(
  query: { q?: string; type?: string; offset?: number; limit?: number },
  enabled = true
) {
  const q = query.q ?? "";
  const type = query.type ?? "";
  const offset = query.offset ?? 0;
  const limit = query.limit ?? 24;
  return useQuery({
    queryKey: portalKeys.search(q, type, offset, limit),
    queryFn: ({ signal }) =>
      searchPortal(
        {
          q: q || undefined,
          type: type || undefined,
          offset,
          limit,
        },
        signal
      ),
    enabled,
    placeholderData: (previous) => previous,
    staleTime: 30_000,
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
