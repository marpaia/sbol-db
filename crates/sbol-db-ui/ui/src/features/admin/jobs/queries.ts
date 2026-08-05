import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { observabilityKeys } from "@/features/admin/observability/queries";
import {
  cancelJob,
  fetchJobAttempts,
  fetchJobLogs,
  fetchRecentJobs,
  getJob,
  type RecentJob,
  type RecentJobsQuery,
} from "./api";

const JOBS_MS = 30_000;
const JOB_DETAIL_MS = 1_000;

export const jobKeys = {
  all: ["admin", "jobs"] as const,
  lists: () => [...jobKeys.all, "list"] as const,
  list: (query: RecentJobsQuery) =>
    [
      ...jobKeys.lists(),
      query.limit ?? null,
      query.queue ?? "",
      query.status ?? "",
    ] as const,
  detail: (id: string) => [...jobKeys.all, "detail", id] as const,
  attempts: (id: string) => [...jobKeys.detail(id), "attempts"] as const,
  logs: (id: string) => [...jobKeys.detail(id), "logs"] as const,
};

export function useRecentJobs(query: RecentJobsQuery = {}) {
  return useQuery({
    queryKey: jobKeys.list(query),
    queryFn: ({ signal }) => fetchRecentJobs(query, signal),
    staleTime: JOBS_MS,
    refetchInterval: JOBS_MS,
    placeholderData: (previous) => previous,
  });
}

export function useJob(id: string) {
  return useQuery({
    queryKey: jobKeys.detail(id),
    queryFn: ({ signal }) => getJob(id, signal),
    enabled: id.length > 0,
    refetchInterval: (query) => {
      const job = query.state.data as RecentJob | undefined;
      if (!job) return JOB_DETAIL_MS;
      return isLiveJobStatus(job.status) ? JOB_DETAIL_MS : false;
    },
    placeholderData: (previous) => previous,
  });
}

export function useCancelJob() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, confirmation }: { id: string; confirmation: string }) =>
      cancelJob(id, confirmation),
    onSuccess: (_data, { id }) => {
      queryClient.invalidateQueries({ queryKey: jobKeys.detail(id) });
      queryClient.invalidateQueries({ queryKey: jobKeys.attempts(id) });
      queryClient.invalidateQueries({ queryKey: jobKeys.lists() });
      queryClient.invalidateQueries({ queryKey: observabilityKeys.summary() });
    },
  });
}

export function useJobAttempts(
  id: string,
  parentStatus: RecentJob["status"] | undefined
) {
  return useQuery({
    queryKey: jobKeys.attempts(id),
    queryFn: ({ signal }) => fetchJobAttempts(id, signal),
    enabled: id.length > 0,
    refetchInterval: isLiveJobStatus(parentStatus) ? JOB_DETAIL_MS : false,
    placeholderData: (previous) => previous,
  });
}

export function useJobLogs(
  id: string,
  parentStatus: RecentJob["status"] | undefined
) {
  return useQuery({
    queryKey: jobKeys.logs(id),
    queryFn: ({ signal }) => fetchJobLogs(id, { limit: 500 }, signal),
    enabled: id.length > 0,
    refetchInterval: isLiveJobStatus(parentStatus) ? JOB_DETAIL_MS : false,
    placeholderData: (previous) => previous,
  });
}

function isLiveJobStatus(status: RecentJob["status"] | undefined): boolean {
  return status === undefined || status === "queued" || status === "running";
}
