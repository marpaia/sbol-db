import { useQuery } from "@tanstack/react-query";

import { fetchOverview } from "@/lib/api";

const FRESH_MS = 30_000;

export function useOverview() {
  return useQuery({
    queryKey: ["lab", "overview"],
    queryFn: ({ signal }) => fetchOverview(signal),
    staleTime: FRESH_MS,
  });
}
