/**
 * Backend identity and feature capabilities. The server reports which
 * features it supports for the active storage backend; the UI reads
 * this once and gates nav, commands, and routes on it. The value never
 * changes for a running server, so it's cached indefinitely.
 */

import { useQuery } from "@tanstack/react-query";

import { fetchLabInfo } from "./api";

export const backendKeys = {
  info: ["admin", "backend", "info"] as const,
};

export function useBackendInfo() {
  return useQuery({
    queryKey: backendKeys.info,
    queryFn: ({ signal }) => fetchLabInfo(signal),
    staleTime: Infinity,
  });
}
