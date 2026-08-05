import { useQuery } from "@tanstack/react-query";

import { fetchObservabilitySummary } from "./api";

const SUMMARY_MS = 5_000;

export const observabilityKeys = {
  all: ["admin", "observability"] as const,
  summary: () => [...observabilityKeys.all, "summary"] as const,
};

export function useObservabilitySummary() {
  return useQuery({
    queryKey: observabilityKeys.summary(),
    queryFn: ({ signal }) => fetchObservabilitySummary(signal),
    staleTime: SUMMARY_MS,
    refetchInterval: SUMMARY_MS,
    placeholderData: (previous) => previous,
  });
}
