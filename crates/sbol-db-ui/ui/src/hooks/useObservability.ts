/** TanStack Query wrappers for the lab observability endpoints. */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import {
  cancelJob,
  fetchJobAttempts,
  fetchJobLogs,
  fetchLsmOverview,
  fetchMaintenanceActivity,
  fetchMaintenanceDatabase,
  fetchMaintenanceIndexes,
  fetchMaintenanceLocks,
  fetchMaintenanceSlowQueries,
  fetchMaintenanceTables,
  fetchMaintenanceTableSchema,
  fetchObservabilitySummary,
  fetchRecentJobs,
  getJob,
  type RecentJob,
  type RecentJobsQuery,
} from "@/lib/api";

const SUMMARY_MS = 5_000;
const MAINTENANCE_MS = 15_000;
const JOBS_MS = 30_000;
const JOB_DETAIL_MS = 1_000;

export function useObservabilitySummary() {
  return useQuery({
    queryKey: ["lab", "obs", "summary"],
    queryFn: ({ signal }) => fetchObservabilitySummary(signal),
    staleTime: SUMMARY_MS,
    refetchInterval: SUMMARY_MS,
    placeholderData: (prev) => prev,
  });
}

export function useMaintenanceDatabase() {
  return useQuery({
    queryKey: ["lab", "obs", "maintenance", "database"],
    queryFn: ({ signal }) => fetchMaintenanceDatabase(signal),
    staleTime: MAINTENANCE_MS,
    refetchInterval: MAINTENANCE_MS,
    placeholderData: (prev) => prev,
  });
}

export function useMaintenanceTables(limit = 20, offset = 0) {
  return useQuery({
    queryKey: ["lab", "obs", "maintenance", "tables", limit, offset],
    queryFn: ({ signal }) => fetchMaintenanceTables(limit, offset, signal),
    staleTime: MAINTENANCE_MS,
    refetchInterval: MAINTENANCE_MS,
    placeholderData: (prev) => prev,
  });
}

export function useMaintenanceIndexes(limit = 30) {
  return useQuery({
    queryKey: ["lab", "obs", "maintenance", "indexes", limit],
    queryFn: ({ signal }) => fetchMaintenanceIndexes(limit, signal),
    staleTime: MAINTENANCE_MS,
    refetchInterval: MAINTENANCE_MS,
    placeholderData: (prev) => prev,
  });
}

export function useMaintenanceActivity(limit = 50) {
  return useQuery({
    queryKey: ["lab", "obs", "maintenance", "activity", limit],
    queryFn: ({ signal }) => fetchMaintenanceActivity(limit, signal),
    staleTime: MAINTENANCE_MS,
    refetchInterval: MAINTENANCE_MS,
    placeholderData: (prev) => prev,
  });
}

export function useMaintenanceLocks() {
  return useQuery({
    queryKey: ["lab", "obs", "maintenance", "locks"],
    queryFn: ({ signal }) => fetchMaintenanceLocks(signal),
    staleTime: MAINTENANCE_MS,
    refetchInterval: MAINTENANCE_MS,
    placeholderData: (prev) => prev,
  });
}

export function useMaintenanceSlowQueries(limit = 20) {
  return useQuery({
    queryKey: ["lab", "obs", "maintenance", "slow-queries", limit],
    queryFn: ({ signal }) => fetchMaintenanceSlowQueries(limit, signal),
    staleTime: MAINTENANCE_MS,
    refetchInterval: MAINTENANCE_MS,
    placeholderData: (prev) => prev,
  });
}

export function useMaintenanceTableSchema(name: string) {
  return useQuery({
    queryKey: ["lab", "obs", "maintenance", "table-schema", name],
    queryFn: ({ signal }) => fetchMaintenanceTableSchema(name, signal),
    enabled: name.length > 0,
    staleTime: 60_000,
    retry: false,
    placeholderData: (prev) => prev,
  });
}

export function useLsmOverview() {
  return useQuery({
    queryKey: ["lab", "obs", "maintenance", "lsm"],
    queryFn: ({ signal }) => fetchLsmOverview(signal),
    staleTime: MAINTENANCE_MS,
    refetchInterval: MAINTENANCE_MS,
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
      if (!job) return JOB_DETAIL_MS;
      return isLiveJobStatus(job.status) ? JOB_DETAIL_MS : false;
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
 * Per-attempt audit log for a job. Like `useJob`, polls every second while
 * the parent job is still pending so a retried attempt becomes visible
 * without manual refresh.
 */
export function useJobAttempts(
  id: string,
  parentStatus: RecentJob["status"] | undefined
) {
  return useQuery({
    queryKey: ["job", id, "attempts"],
    queryFn: ({ signal }) => fetchJobAttempts(id, signal),
    enabled: id.length > 0,
    refetchInterval: isLiveJobStatus(parentStatus) ? JOB_DETAIL_MS : false,
    placeholderData: (prev) => prev,
  });
}

export function useJobLogs(
  id: string,
  parentStatus: RecentJob["status"] | undefined
) {
  return useQuery({
    queryKey: ["job", id, "logs"],
    queryFn: ({ signal }) => fetchJobLogs(id, { limit: 500 }, signal),
    enabled: id.length > 0,
    refetchInterval: isLiveJobStatus(parentStatus) ? JOB_DETAIL_MS : false,
    placeholderData: (prev) => prev,
  });
}

function isLiveJobStatus(status: RecentJob["status"] | undefined): boolean {
  return status === undefined || status === "queued" || status === "running";
}
