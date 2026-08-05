import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { QueryClient } from "@tanstack/react-query";

import { auditKeys } from "@/features/admin/settings/audit/queries";
import {
  deletePlugin,
  deleteRegistry,
  deleteRemote,
  fetchAdminIntegrations,
  joinFederation,
  savePlugin,
  saveRegistry,
  saveRemote,
  syncFederation,
} from "./api";

export const integrationKeys = {
  all: ["admin", "settings", "integrations"] as const,
};

export function useAdminIntegrations() {
  return useQuery({
    queryKey: integrationKeys.all,
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

function invalidateIntegrations(client: QueryClient) {
  client.invalidateQueries({ queryKey: integrationKeys.all });
  client.invalidateQueries({ queryKey: auditKeys.all });
}
