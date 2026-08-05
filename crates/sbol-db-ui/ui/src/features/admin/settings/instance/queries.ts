import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { auditKeys } from "@/features/admin/settings/audit/queries";
import { instanceKeys as publicInstanceKeys } from "@/features/instance/queries";
import { fetchAdminInstance, updateAdminInstance } from "./api";

export const adminInstanceKeys = {
  all: ["admin", "settings", "instance"] as const,
};

export function useAdminInstance() {
  return useQuery({
    queryKey: adminInstanceKeys.all,
    queryFn: ({ signal }) => fetchAdminInstance(signal),
  });
}

export function useUpdateAdminInstance() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: updateAdminInstance,
    onSuccess: () => {
      client.invalidateQueries({ queryKey: adminInstanceKeys.all });
      client.invalidateQueries({ queryKey: publicInstanceKeys.all });
      client.invalidateQueries({ queryKey: auditKeys.all });
    },
  });
}
