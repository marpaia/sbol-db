import { useQuery } from "@tanstack/react-query";

import { fetchAccount, fetchSharedObjects } from "./api";

export const accountKeys = {
  all: ["registry", "account"] as const,
  profile: () => [...accountKeys.all, "profile"] as const,
  shared: () => [...accountKeys.all, "shared"] as const,
  sharedPage: (offset: number, limit: number) =>
    [...accountKeys.shared(), offset, limit] as const,
};

export function useAccount(enabled = true) {
  return useQuery({
    queryKey: accountKeys.profile(),
    queryFn: ({ signal }) => fetchAccount(signal),
    enabled,
    staleTime: 15_000,
    retry: false,
  });
}

export function useSharedObjects(offset = 0, limit = 24, enabled = true) {
  return useQuery({
    queryKey: accountKeys.sharedPage(offset, limit),
    queryFn: ({ signal }) => fetchSharedObjects({ offset, limit }, signal),
    enabled,
    staleTime: 15_000,
    retry: false,
  });
}
