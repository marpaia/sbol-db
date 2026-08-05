import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { QueryClient } from "@tanstack/react-query";

import { auditKeys } from "@/features/admin/settings/audit/queries";
import {
  createAdminUser,
  deleteAdminUser,
  fetchAdminUsers,
  updateAdminUser,
  type UpdateAdminUser,
} from "./api";

export const adminUserKeys = {
  all: ["admin", "settings", "users"] as const,
};

export function useAdminUsers() {
  return useQuery({
    queryKey: adminUserKeys.all,
    queryFn: ({ signal }) => fetchAdminUsers(signal),
  });
}

export function useCreateAdminUser() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: createAdminUser,
    onSuccess: () => invalidateUsers(client),
  });
}

export function useUpdateAdminUser() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({
      username,
      payload,
    }: {
      username: string;
      payload: UpdateAdminUser;
    }) => updateAdminUser(username, payload),
    onSuccess: () => invalidateUsers(client),
  });
}

export function useDeleteAdminUser() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({
      username,
      confirmation,
    }: {
      username: string;
      confirmation: string;
    }) => deleteAdminUser(username, confirmation),
    onSuccess: () => invalidateUsers(client),
  });
}

function invalidateUsers(client: QueryClient) {
  client.invalidateQueries({ queryKey: adminUserKeys.all });
  client.invalidateQueries({ queryKey: auditKeys.all });
}
