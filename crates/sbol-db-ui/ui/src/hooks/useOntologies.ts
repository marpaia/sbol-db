/** TanStack Query wrappers for the ontology endpoints. */

import { useQuery } from "@tanstack/react-query";

import {
  fetchOntologyDescendants,
  fetchOntologyTerm,
  listOntologies,
} from "@/lib/api";

const FRESH_MS = 60_000;

export function useOntologies() {
  return useQuery({
    queryKey: ["lab", "ontologies"],
    queryFn: ({ signal }) => listOntologies(signal),
    staleTime: FRESH_MS,
  });
}

export function useOntologyTerm(iri: string) {
  return useQuery({
    queryKey: ["lab", "ontology", "term", iri],
    queryFn: ({ signal }) => fetchOntologyTerm(iri, signal),
    enabled: iri.length > 0,
    staleTime: FRESH_MS,
    retry: false,
  });
}

export function useOntologyDescendants(iri: string, enabled: boolean) {
  return useQuery({
    queryKey: ["lab", "ontology", "descendants", iri],
    queryFn: ({ signal }) => fetchOntologyDescendants(iri, signal),
    enabled: enabled && iri.length > 0,
    staleTime: FRESH_MS,
    retry: false,
  });
}
