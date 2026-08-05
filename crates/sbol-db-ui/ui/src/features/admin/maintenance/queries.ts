import { useQuery } from "@tanstack/react-query";

import {
  fetchLsmOverview,
  fetchMaintenanceActivity,
  fetchMaintenanceDatabase,
  fetchMaintenanceIndexes,
  fetchMaintenanceLocks,
  fetchMaintenanceSlowQueries,
  fetchMaintenanceTables,
  fetchMaintenanceTableSchema,
} from "./api";

const MAINTENANCE_MS = 15_000;

export const maintenanceKeys = {
  all: ["admin", "maintenance"] as const,
  database: () => [...maintenanceKeys.all, "database"] as const,
  tables: (limit: number, offset: number) =>
    [...maintenanceKeys.all, "tables", limit, offset] as const,
  indexes: (limit: number) =>
    [...maintenanceKeys.all, "indexes", limit] as const,
  activity: (limit: number) =>
    [...maintenanceKeys.all, "activity", limit] as const,
  locks: () => [...maintenanceKeys.all, "locks"] as const,
  slowQueries: (limit: number) =>
    [...maintenanceKeys.all, "slow-queries", limit] as const,
  tableSchema: (name: string) =>
    [...maintenanceKeys.all, "table-schema", name] as const,
  lsm: () => [...maintenanceKeys.all, "lsm"] as const,
};

export function useMaintenanceDatabase() {
  return useQuery({
    queryKey: maintenanceKeys.database(),
    queryFn: ({ signal }) => fetchMaintenanceDatabase(signal),
    staleTime: MAINTENANCE_MS,
    refetchInterval: MAINTENANCE_MS,
    placeholderData: (previous) => previous,
  });
}

export function useMaintenanceTables(limit = 20, offset = 0) {
  return useQuery({
    queryKey: maintenanceKeys.tables(limit, offset),
    queryFn: ({ signal }) => fetchMaintenanceTables(limit, offset, signal),
    staleTime: MAINTENANCE_MS,
    refetchInterval: MAINTENANCE_MS,
    placeholderData: (previous) => previous,
  });
}

export function useMaintenanceIndexes(limit = 30) {
  return useQuery({
    queryKey: maintenanceKeys.indexes(limit),
    queryFn: ({ signal }) => fetchMaintenanceIndexes(limit, signal),
    staleTime: MAINTENANCE_MS,
    refetchInterval: MAINTENANCE_MS,
    placeholderData: (previous) => previous,
  });
}

export function useMaintenanceActivity(limit = 50) {
  return useQuery({
    queryKey: maintenanceKeys.activity(limit),
    queryFn: ({ signal }) => fetchMaintenanceActivity(limit, signal),
    staleTime: MAINTENANCE_MS,
    refetchInterval: MAINTENANCE_MS,
    placeholderData: (previous) => previous,
  });
}

export function useMaintenanceLocks() {
  return useQuery({
    queryKey: maintenanceKeys.locks(),
    queryFn: ({ signal }) => fetchMaintenanceLocks(signal),
    staleTime: MAINTENANCE_MS,
    refetchInterval: MAINTENANCE_MS,
    placeholderData: (previous) => previous,
  });
}

export function useMaintenanceSlowQueries(limit = 20) {
  return useQuery({
    queryKey: maintenanceKeys.slowQueries(limit),
    queryFn: ({ signal }) => fetchMaintenanceSlowQueries(limit, signal),
    staleTime: MAINTENANCE_MS,
    refetchInterval: MAINTENANCE_MS,
    placeholderData: (previous) => previous,
  });
}

export function useMaintenanceTableSchema(name: string) {
  return useQuery({
    queryKey: maintenanceKeys.tableSchema(name),
    queryFn: ({ signal }) => fetchMaintenanceTableSchema(name, signal),
    enabled: name.length > 0,
    staleTime: 60_000,
    retry: false,
    placeholderData: (previous) => previous,
  });
}

export function useLsmOverview() {
  return useQuery({
    queryKey: maintenanceKeys.lsm(),
    queryFn: ({ signal }) => fetchLsmOverview(signal),
    staleTime: MAINTENANCE_MS,
    refetchInterval: MAINTENANCE_MS,
    placeholderData: (previous) => previous,
  });
}
