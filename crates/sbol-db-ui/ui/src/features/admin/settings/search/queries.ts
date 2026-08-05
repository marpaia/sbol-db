import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { auditKeys } from "@/features/admin/settings/audit/queries";
import { fetchSearchStatus, rebuildSearch } from "./api";

export const searchStatusKeys = {
  all: ["admin", "settings", "search"] as const,
};

export function useSearchStatus() {
  return useQuery({
    queryKey: searchStatusKeys.all,
    queryFn: ({ signal }) => fetchSearchStatus(signal),
    refetchInterval: 10_000,
  });
}

export function useRebuildSearch() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: rebuildSearch,
    onSuccess: () => {
      client.invalidateQueries({ queryKey: searchStatusKeys.all });
      client.invalidateQueries({ queryKey: auditKeys.all });
    },
  });
}
