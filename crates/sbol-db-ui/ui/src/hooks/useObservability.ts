/** TanStack Query wrappers for the lab observability endpoints. */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import {
  cancelJob,
  fetchJobAttempts,
  fetchObservabilitySummary,
  fetchPgActivity,
  fetchPgDatabase,
  fetchPgIndexes,
  fetchPgLocks,
  fetchPgSlowQueries,
  fetchPgTables,
  fetchPgTableSchema,
  fetchRecentJobs,
  getJob,
  type RecentJob,
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

/**
 * Single-job detail fetch. While the job is still pending (queued or
 * running), poll on a short interval so the page reflects worker
 * progress without a manual refresh; stop polling once the job
 * reaches a terminal state.
 */
export function useJob(id: string) {
  return useQuery({
    queryKey: ["job", id],
    queryFn: ({ signal }) => getJob(id, signal),
    enabled: id.length > 0,
    refetchInterval: (q) => {
      const job = q.state.data as RecentJob | undefined;
      if (!job) return 5_000;
      return job.status === "queued" || job.status === "running"
        ? 5_000
        : false;
    },
    placeholderData: (prev) => prev,
  });
}

export function useCancelJob() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => cancelJob(id),
    onSuccess: (_data, id) => {
      qc.invalidateQueries({ queryKey: ["job", id] });
      qc.invalidateQueries({ queryKey: ["job", id, "attempts"] });
      qc.invalidateQueries({ queryKey: ["lab", "obs", "jobs", "recent"] });
      qc.invalidateQueries({ queryKey: ["lab", "obs", "summary"] });
    },
  });
}

/**
 * Per-attempt audit log for a job. Like `useJob`, polls every 5 s while
 * the parent job is still pending so a re-tried attempt becomes visible
 * without manual refresh.
 */
export function useJobAttempts(id: string, parentStatus: RecentJob["status"] | undefined) {
  return useQuery({
    queryKey: ["job", id, "attempts"],
    queryFn: ({ signal }) => fetchJobAttempts(id, signal),
    enabled: id.length > 0,
    refetchInterval:
      parentStatus === "queued" || parentStatus === "running" ? 5_000 : false,
    placeholderData: (prev) => prev,
  });
}
