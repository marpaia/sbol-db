/** TanStack Query wrappers for the ontology endpoints. */

import { useQuery } from "@tanstack/react-query";

import {
  fetchOntologyDescendants,
  fetchOntologyTerm,
  listOntologies,
  listOntologyTerms,
  type OntologyTermsQuery,
} from "./api";

export const ontologyKeys = {
  all: ["admin", "ontologies"] as const,
  terms: () => [...ontologyKeys.all, "terms"] as const,
  term: (iri: string) => [...ontologyKeys.terms(), "detail", iri] as const,
  descendants: (iri: string) =>
    [...ontologyKeys.term(iri), "descendants"] as const,
  list: (query: OntologyTermsQuery) =>
    [
      ...ontologyKeys.terms(),
      "list",
      query.prefix,
      query.q ?? "",
      query.limit ?? null,
      query.offset ?? 0,
    ] as const,
};

const FRESH_MS = 60_000;

export function useOntologies() {
  return useQuery({
    queryKey: ontologyKeys.all,
    queryFn: ({ signal }) => listOntologies(signal),
    staleTime: FRESH_MS,
  });
}

export function useOntologyTerm(iri: string) {
  return useQuery({
    queryKey: ontologyKeys.term(iri),
    queryFn: ({ signal }) => fetchOntologyTerm(iri, signal),
    enabled: iri.length > 0,
    staleTime: FRESH_MS,
    retry: false,
  });
}

export function useOntologyDescendants(iri: string, enabled: boolean) {
  return useQuery({
    queryKey: ontologyKeys.descendants(iri),
    queryFn: ({ signal }) => fetchOntologyDescendants(iri, signal),
    enabled: enabled && iri.length > 0,
    staleTime: FRESH_MS,
    retry: false,
  });
}

export function useOntologyTerms(query: OntologyTermsQuery) {
  return useQuery({
    queryKey: ontologyKeys.list(query),
    queryFn: ({ signal }) => listOntologyTerms(query, signal),
    enabled: query.prefix.length > 0,
    staleTime: FRESH_MS,
    placeholderData: (prev) => prev,
  });
}
