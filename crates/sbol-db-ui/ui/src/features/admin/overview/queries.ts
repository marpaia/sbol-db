import { useQuery } from "@tanstack/react-query";

import { fetchOverview } from "./api";

export const overviewKeys = {
  all: ["admin", "overview"] as const,
};

const FRESH_MS = 30_000;

export function useOverview() {
  return useQuery({
    queryKey: overviewKeys.all,
    queryFn: ({ signal }) => fetchOverview(signal),
    staleTime: FRESH_MS,
  });
}
