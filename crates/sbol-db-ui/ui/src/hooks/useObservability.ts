/** TanStack Query wrappers for the lab observability endpoints. */

import { useQuery } from "@tanstack/react-query";

import {
  fetchObservabilitySummary,
  fetchPgActivity,
  fetchPgDatabase,
  fetchPgIndexes,
  fetchPgLocks,
  fetchPgSlowQueries,
  fetchPgTables,
  fetchPgTableSchema,
  fetchRecentJobs,
  type RecentJobsQuery,
} from "@/lib/api";

const SUMMARY_MS = 5_000;
const POSTGRES_MS = 15_000;
const JOBS_MS = 30_000;

export function useObservabilitySummary() {
  return useQuery({
    queryKey: ["lab", "obs", "summary"],
    queryFn: ({ signal }) => fetchObservabilitySummary(signal),
    staleTime: SUMMARY_MS,
    refetchInterval: SUMMARY_MS,
    placeholderData: (prev) => prev,
  });
}

export function usePgDatabase() {
  return useQuery({
    queryKey: ["lab", "obs", "pg", "database"],
    queryFn: ({ signal }) => fetchPgDatabase(signal),
    staleTime: POSTGRES_MS,
    refetchInterval: POSTGRES_MS,
    placeholderData: (prev) => prev,
  });
}

export function usePgTables(limit = 20, offset = 0) {
  return useQuery({
    queryKey: ["lab", "obs", "pg", "tables", limit, offset],
    queryFn: ({ signal }) => fetchPgTables(limit, offset, signal),
    staleTime: POSTGRES_MS,
    refetchInterval: POSTGRES_MS,
    placeholderData: (prev) => prev,
  });
}

export function usePgIndexes(limit = 30) {
  return useQuery({
    queryKey: ["lab", "obs", "pg", "indexes", limit],
    queryFn: ({ signal }) => fetchPgIndexes(limit, signal),
    staleTime: POSTGRES_MS,
    refetchInterval: POSTGRES_MS,
    placeholderData: (prev) => prev,
  });
}

export function usePgActivity(limit = 50) {
  return useQuery({
    queryKey: ["lab", "obs", "pg", "activity", limit],
    queryFn: ({ signal }) => fetchPgActivity(limit, signal),
    staleTime: POSTGRES_MS,
    refetchInterval: POSTGRES_MS,
    placeholderData: (prev) => prev,
  });
}

export function usePgLocks() {
  return useQuery({
    queryKey: ["lab", "obs", "pg", "locks"],
    queryFn: ({ signal }) => fetchPgLocks(signal),
    staleTime: POSTGRES_MS,
    refetchInterval: POSTGRES_MS,
    placeholderData: (prev) => prev,
  });
}

export function usePgSlowQueries(limit = 20) {
  return useQuery({
    queryKey: ["lab", "obs", "pg", "slow-queries", limit],
    queryFn: ({ signal }) => fetchPgSlowQueries(limit, signal),
    staleTime: POSTGRES_MS,
    refetchInterval: POSTGRES_MS,
    placeholderData: (prev) => prev,
  });
}

export function usePgTableSchema(name: string) {
  return useQuery({
    queryKey: ["lab", "obs", "pg", "table-schema", name],
    queryFn: ({ signal }) => fetchPgTableSchema(name, signal),
    enabled: name.length > 0,
    staleTime: 60_000,
    retry: false,
    placeholderData: (prev) => prev,
  });
}

export function useRecentJobs(query: RecentJobsQuery = {}) {
  return useQuery({
    queryKey: [
      "lab",
      "obs",
      "jobs",
      "recent",
      query.limit ?? null,
      query.queue ?? "",
      query.status ?? "",
    ],
    queryFn: ({ signal }) => fetchRecentJobs(query, signal),
    staleTime: JOBS_MS,
    refetchInterval: JOBS_MS,
    placeholderData: (prev) => prev,
  });
}
