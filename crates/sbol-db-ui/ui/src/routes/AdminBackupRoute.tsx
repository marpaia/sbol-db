import {
  Cloud,
  DatabaseBackup,
  ExternalLink,
  HardDrive,
  History,
  ShieldCheck,
} from "lucide-react";
import { useNavigate } from "react-router-dom";

import {
  AdminPage,
  AdminSection,
  MutationStatus,
} from "@/components/admin/AdminPage";
import { ErrorBanner } from "@/components/lab/ErrorBanner";
import { JobStatusBadge } from "@/components/observability/JobStatusBadge";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  useCompleteBackupStatus,
  useTriggerCompleteBackup,
} from "@/features/admin/queries";
import type { RecentJob } from "@/lib/api";
import { adminPath } from "@/lib/routes";
import { describeError, formatBytes, formatRelative } from "@/lib/utils";

interface CompletedBackupResult {
  backup_id?: string;
  artifact_bytes?: number;
  artifact_sha256?: string;
  verified_at?: string;
  trigger?: "manual" | "scheduled" | "pre_deploy";
  remote?: {
    provider?: string;
    bucket?: string;
    object_key?: string;
    verified_at?: string;
  } | null;
}

export default function AdminBackupRoute() {
  const navigate = useNavigate();
  const status = useCompleteBackupStatus();
  const trigger = useTriggerCompleteBackup();
  const latestVerified = status.data?.recent.find(
    (job) => job.status === "succeeded"
  );
  const latestResult = backupResult(latestVerified);

  return (
    <AdminPage
      title="Backups and recovery"
      description="Create, verify, and publish one complete encrypted recovery artifact containing RocksDB, attachments, search state, and ACME state. Manual, scheduled, and deployment backups use this same workflow."
      action={
        <Button
          disabled={!status.data?.enabled || trigger.isPending}
          onClick={() => trigger.mutate()}
        >
          <DatabaseBackup />
          {trigger.isPending ? "Queueing…" : "Create complete backup"}
        </Button>
      }
    >
      {status.error ? (
        <ErrorBanner
          title="Couldn't load backup status"
          body={describeError(status.error)}
        />
      ) : (
        <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
          <StatusCard
            icon={<ShieldCheck />}
            label="Backup service"
            value={status.data?.enabled ? "Ready" : "Unavailable"}
            detail={
              status.data?.enabled
                ? "Encrypted checkpoint executor is installed"
                : "Requires the production RocksDB runtime"
            }
            tone={status.data?.enabled ? "success" : "warning"}
          />
          <StatusCard
            icon={<Cloud />}
            label="Remote verification"
            value={latestResult?.remote ? "Verified" : "No verified backup"}
            detail={
              latestResult?.remote?.verified_at
                ? formatRelative(latestResult.remote.verified_at)
                : "A backup is successful only after object-store readback"
            }
            tone={latestResult?.remote ? "success" : "neutral"}
          />
          <StatusCard
            icon={<History />}
            label="Latest complete backup"
            value={
              latestVerified
                ? formatRelative(latestVerified.finished_at)
                : "None yet"
            }
            detail={
              latestResult?.trigger?.replace("_", " ") ?? "No completed jobs"
            }
          />
          <StatusCard
            icon={<HardDrive />}
            label="Latest artifact"
            value={formatBytes(latestResult?.artifact_bytes)}
            detail={
              latestResult?.artifact_sha256
                ? `SHA-256 ${latestResult.artifact_sha256.slice(0, 12)}…`
                : "Encrypted before leaving this server"
            }
          />
        </div>
      )}

      <MutationStatus
        pending={trigger.isPending}
        error={trigger.error}
        success={
          trigger.data
            ? trigger.data.deduplicated
              ? `Backup job ${shortId(trigger.data.job.id)} was already queued.`
              : `Backup job ${shortId(trigger.data.job.id)} queued.`
            : null
        }
      />

      <AdminSection
        title="Complete recovery contract"
        description="Every trigger produces the same artifact and passes the same local verification, remote upload, remote readback, and semantic verification gates."
      >
        <div className="flex flex-wrap gap-2">
          {(
            status.data?.components ?? ["rocksdb", "blobs", "search", "acme"]
          ).map((component) => (
            <Badge key={component} variant="outline" className="capitalize">
              {component}
            </Badge>
          ))}
        </div>
        <p className="mt-4 max-w-3xl text-xs leading-5 text-muted-foreground">
          The recovery identity is held outside this server. Object-store
          credentials come from the host's workload identity or provider
          environment and are never returned to the browser.
        </p>
      </AdminSection>

      <AdminSection
        title="Recent backup jobs"
        description="Open a job to inspect its attempts, logs, encrypted artifact checksum, disk preflight, object-store location, and remote verification result."
      >
        {!status.data ? (
          <p className="text-sm text-muted-foreground">
            Loading backup history…
          </p>
        ) : status.data.recent.length === 0 ? (
          <div className="rounded-lg border border-dashed px-5 py-8 text-center">
            <DatabaseBackup className="mx-auto size-5 text-muted-foreground" />
            <p className="mt-3 text-sm font-medium">No complete backups yet</p>
            <p className="mt-1 text-xs text-muted-foreground">
              Create one now or wait for the configured schedule.
            </p>
          </div>
        ) : (
          <div className="divide-y overflow-hidden rounded-lg border">
            {status.data.recent.map((job) => (
              <BackupJobRow
                key={job.id}
                job={job}
                onOpen={() =>
                  navigate(adminPath(`/observability/jobs/${job.id}`))
                }
              />
            ))}
          </div>
        )}
      </AdminSection>

      <AdminSection
        title="Recovery activation"
        description="Verification, activation, and rollback run offline so an open RocksDB process can never be replaced underneath live traffic."
      >
        <div className="rounded-lg border border-warning/30 bg-warning/5 p-4">
          <p className="text-sm font-medium">
            The running admin UI cannot activate a restore.
          </p>
          <p className="mt-1 max-w-3xl text-xs leading-5 text-muted-foreground">
            Stop sbol-db, retrieve the selected encrypted artifact and recovery
            identity, then use the recovery commands on the server. Runtime
            configuration and recovery state will be shown here as the edge
            management phase is completed.
          </p>
        </div>
      </AdminSection>
    </AdminPage>
  );
}

function BackupJobRow({ job, onOpen }: { job: RecentJob; onOpen: () => void }) {
  const result = backupResult(job);
  const payload = asRecord(job.payload);
  const trigger =
    typeof payload?.trigger === "string"
      ? payload.trigger.replace("_", " ")
      : "unknown";
  return (
    <button
      type="button"
      onClick={onOpen}
      className="group grid w-full gap-3 bg-background px-4 py-3 text-left transition-colors hover:bg-accent/50 sm:grid-cols-[minmax(0,1.3fr)_minmax(0,1fr)_auto] sm:items-center"
    >
      <div className="min-w-0">
        <div className="flex flex-wrap items-center gap-2">
          <JobStatusBadge status={job.status} />
          <span className="text-xs capitalize text-muted-foreground">
            {trigger}
          </span>
        </div>
        <p className="mt-1 truncate font-mono text-[11px] text-muted-foreground">
          {job.id}
        </p>
      </div>
      <div className="min-w-0 text-xs text-muted-foreground">
        {result?.remote ? (
          <>
            <p className="truncate text-foreground">
              {result.remote.provider?.toUpperCase()} · {result.remote.bucket}
            </p>
            <p className="truncate">{result.remote.object_key}</p>
          </>
        ) : job.error ? (
          <p className="line-clamp-2 text-destructive">{job.error}</p>
        ) : (
          <p>
            {job.status === "succeeded"
              ? "Local artifact verified"
              : "Awaiting result"}
          </p>
        )}
      </div>
      <div className="flex items-center justify-between gap-3 text-xs text-muted-foreground sm:justify-end">
        <span>{formatRelative(job.finished_at ?? job.created_at)}</span>
        <ExternalLink className="size-3.5 opacity-50 transition-opacity group-hover:opacity-100" />
      </div>
    </button>
  );
}

function StatusCard({
  icon,
  label,
  value,
  detail,
  tone = "neutral",
}: {
  icon: React.ReactNode;
  label: string;
  value: string;
  detail: string;
  tone?: "neutral" | "success" | "warning";
}) {
  const iconTone =
    tone === "success"
      ? "text-success"
      : tone === "warning"
        ? "text-warning"
        : "text-muted-foreground";
  return (
    <div className="rounded-xl border bg-card p-4 shadow-sm">
      <div
        className={`flex size-8 items-center justify-center rounded-lg bg-muted ${iconTone}`}
      >
        {icon}
      </div>
      <p className="mt-4 text-[11px] uppercase tracking-wide text-muted-foreground">
        {label}
      </p>
      <p className="mt-1 text-sm font-semibold">{value}</p>
      <p className="mt-1 text-[11px] leading-4 text-muted-foreground">
        {detail}
      </p>
    </div>
  );
}

function backupResult(
  job: RecentJob | undefined
): CompletedBackupResult | null {
  return asRecord(job?.result) as CompletedBackupResult | null;
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function shortId(value: string): string {
  return value.slice(0, 8);
}
