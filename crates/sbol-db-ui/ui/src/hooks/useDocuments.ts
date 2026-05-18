/** TanStack Query wrappers for the documents endpoints. */

import { useQuery } from "@tanstack/react-query";

import { getDocument, listDocuments, type DocumentsListQuery } from "@/lib/api";

const FRESH_MS = 30_000;

export function useDocuments(query: DocumentsListQuery = {}) {
  return useQuery({
    queryKey: ["lab", "documents", query.limit ?? null, query.offset ?? 0],
    queryFn: ({ signal }) => listDocuments(query, signal),
    staleTime: FRESH_MS,
    placeholderData: (prev) => prev,
  });
}

export function useDocument(id: string) {
  return useQuery({
    queryKey: ["lab", "documents", "detail", id],
    queryFn: ({ signal }) => getDocument(id, signal),
    enabled: id.length > 0,
    staleTime: FRESH_MS,
    retry: false,
  });
}
