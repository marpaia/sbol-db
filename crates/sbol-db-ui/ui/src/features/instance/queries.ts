import { useQuery } from "@tanstack/react-query";

import { fetchInstance } from "./api";

export const instanceKeys = {
  all: ["instance"] as const,
  detail: () => [...instanceKeys.all, "detail"] as const,
};

export function useInstance() {
  return useQuery({
    queryKey: instanceKeys.detail(),
    queryFn: ({ signal }) => fetchInstance(signal),
    staleTime: 60_000,
  });
}
