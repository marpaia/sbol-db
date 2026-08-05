import { useQuery } from "@tanstack/react-query";

import { fetchSession } from "./api";

export const sessionKeys = {
  all: ["session"] as const,
  detail: () => [...sessionKeys.all, "detail"] as const,
};

export function useSession() {
  return useQuery({
    queryKey: sessionKeys.detail(),
    queryFn: ({ signal }) => fetchSession(signal),
    staleTime: 15_000,
    retry: false,
  });
}
