import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { jobKeys } from "@/features/admin/jobs/queries";
import { observabilityKeys } from "@/features/admin/observability/queries";
import { auditKeys } from "@/features/admin/settings/audit/queries";
import { fetchCompleteBackupStatus, triggerCompleteBackup } from "./api";

export const backupKeys = {
  all: ["admin", "settings", "backup"] as const,
};

export function useCompleteBackupStatus() {
  return useQuery({
    queryKey: backupKeys.all,
    queryFn: ({ signal }) => fetchCompleteBackupStatus(signal),
    refetchInterval: 5_000,
    retry: false,
  });
}

export function useTriggerCompleteBackup() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: () => triggerCompleteBackup("manual"),
    onSuccess: () => {
      client.invalidateQueries({ queryKey: backupKeys.all });
      client.invalidateQueries({ queryKey: auditKeys.all });
      client.invalidateQueries({ queryKey: jobKeys.lists() });
      client.invalidateQueries({ queryKey: observabilityKeys.summary() });
    },
  });
}
