/**
 * Backend identity and feature capabilities. The server reports which
 * features it supports for the active storage backend; the UI reads
 * this once and gates nav, commands, and routes on it. The value never
 * changes for a running server, so it's cached indefinitely.
 */

import { useQuery } from "@tanstack/react-query";

import { fetchLabInfo } from "@/lib/api";

export function useBackendInfo() {
  return useQuery({
    queryKey: ["lab", "info"],
    queryFn: ({ signal }) => fetchLabInfo(signal),
    staleTime: Infinity,
  });
}
