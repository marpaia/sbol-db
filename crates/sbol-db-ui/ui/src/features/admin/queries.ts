import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import {
  createAdminUser,
  deleteAdminUser,
  deletePlugin,
  deleteRegistry,
  deleteRemote,
  fetchAdminAudit,
  fetchAdminInstance,
  fetchAdminIntegrations,
  fetchAdminOverview,
  fetchAdminUsers,
  fetchSearchStatus,
  joinFederation,
  rebuildSearch,
  restoreBackup,
  savePlugin,
  saveRegistry,
  saveRemote,
  syncFederation,
  updateAdminInstance,
  updateAdminUser,
  validateBackup,
  type BackupArchive,
  type UpdateAdminUser,
} from "./api";

export const adminKeys = {
  overview: ["admin", "overview"] as const,
  instance: ["admin", "instance"] as const,
  users: ["admin", "users"] as const,
  integrations: ["admin", "integrations"] as const,
  search: ["admin", "search"] as const,
  audit: ["admin", "audit"] as const,
};

export function useAdminOverview() {
  return useQuery({
    queryKey: adminKeys.overview,
    queryFn: ({ signal }) => fetchAdminOverview(signal),
    staleTime: 60_000,
  });
}

export function useAdminInstance() {
  return useQuery({
    queryKey: adminKeys.instance,
    queryFn: ({ signal }) => fetchAdminInstance(signal),
  });
}

export function useUpdateAdminInstance() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: updateAdminInstance,
    onSuccess: () => {
      client.invalidateQueries({ queryKey: adminKeys.instance });
      client.invalidateQueries({ queryKey: ["portal", "instance"] });
      client.invalidateQueries({ queryKey: adminKeys.audit });
    },
  });
}

export function useAdminUsers() {
  return useQuery({
    queryKey: adminKeys.users,
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

export function useAdminIntegrations() {
  return useQuery({
    queryKey: adminKeys.integrations,
    queryFn: ({ signal }) => fetchAdminIntegrations(signal),
  });
}

export function useJoinFederation() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({
      administratorEmail,
      url,
    }: {
      administratorEmail: string;
      url: string;
    }) => joinFederation(administratorEmail, url),
    onSuccess: () => invalidateIntegrations(client),
  });
}

export function useSyncFederation() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: syncFederation,
    onSuccess: () => invalidateIntegrations(client),
  });
}

export function useSaveRegistry() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ uri, url }: { uri: string; url: string }) =>
      saveRegistry(uri, url),
    onSuccess: () => invalidateIntegrations(client),
  });
}

export function useDeleteRegistry() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({
      uri,
      confirmation,
    }: {
      uri: string;
      confirmation: string;
    }) => deleteRegistry(uri, confirmation),
    onSuccess: () => invalidateIntegrations(client),
  });
}

export function useSaveRemote() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: saveRemote,
    onSuccess: () => invalidateIntegrations(client),
  });
}

export function useDeleteRemote() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ id, confirmation }: { id: string; confirmation: string }) =>
      deleteRemote(id, confirmation),
    onSuccess: () => invalidateIntegrations(client),
  });
}

export function useSavePlugin() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: savePlugin,
    onSuccess: () => invalidateIntegrations(client),
  });
}

export function useDeletePlugin() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({
      category,
      id,
      confirmation,
    }: {
      category: string;
      id: string;
      confirmation: string;
    }) => deletePlugin(category, id, confirmation),
    onSuccess: () => invalidateIntegrations(client),
  });
}

export function useSearchStatus() {
  return useQuery({
    queryKey: adminKeys.search,
    queryFn: ({ signal }) => fetchSearchStatus(signal),
    refetchInterval: 10_000,
  });
}

export function useRebuildSearch() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: rebuildSearch,
    onSuccess: () => {
      client.invalidateQueries({ queryKey: adminKeys.search });
      client.invalidateQueries({ queryKey: adminKeys.audit });
    },
  });
}

export function useAdminAudit() {
  return useQuery({
    queryKey: adminKeys.audit,
    queryFn: ({ signal }) => fetchAdminAudit(signal),
  });
}

export function useValidateBackup() {
  return useMutation({ mutationFn: validateBackup });
}

export function useRestoreBackup() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({
      archive,
      confirmation,
    }: {
      archive: BackupArchive;
      confirmation: string;
    }) => restoreBackup(archive, confirmation),
    onSuccess: () => {
      client.invalidateQueries({ queryKey: adminKeys.search });
      client.invalidateQueries({ queryKey: adminKeys.audit });
      client.invalidateQueries({ queryKey: ["lab", "overview"] });
      client.invalidateQueries({ queryKey: ["lab", "graphs"] });
    },
  });
}

function invalidateUsers(client: ReturnType<typeof useQueryClient>) {
  client.invalidateQueries({ queryKey: adminKeys.users });
  client.invalidateQueries({ queryKey: adminKeys.audit });
}

function invalidateIntegrations(client: ReturnType<typeof useQueryClient>) {
  client.invalidateQueries({ queryKey: adminKeys.integrations });
  client.invalidateQueries({ queryKey: adminKeys.audit });
}
