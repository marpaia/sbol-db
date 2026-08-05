import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { auditKeys } from "@/features/admin/settings/audit/queries";
import { backupKeys } from "@/features/admin/settings/backup/queries";
import { fetchEdgeAdmin, updateEdgeAdmin } from "./api";

export const edgeKeys = {
  all: ["admin", "settings", "edge"] as const,
};

export function useEdgeAdmin(enabled = true) {
  return useQuery({
    queryKey: edgeKeys.all,
    queryFn: ({ signal }) => fetchEdgeAdmin(signal),
    enabled,
    refetchInterval: enabled ? 10_000 : false,
    retry: false,
  });
}

export function useUpdateEdgeAdmin() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: updateEdgeAdmin,
    onSuccess: () => {
      client.invalidateQueries({ queryKey: edgeKeys.all });
      client.invalidateQueries({ queryKey: backupKeys.all });
      client.invalidateQueries({ queryKey: auditKeys.all });
    },
  });
}
