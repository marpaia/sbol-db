import {
  Cloud,
  HardDrive,
  KeyRound,
  RefreshCcw,
  Save,
  ShieldCheck,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";

import {
  AdminPage,
  AdminSection,
  Field,
  MutationStatus,
} from "@/components/admin/AdminPage";
import { SurfaceState } from "@/components/portal/SurfaceState";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { useEdgeAdmin, useUpdateEdgeAdmin } from "@/features/admin/queries";
import type { EdgeSettings } from "@/features/admin/api";
import { formatBytes, formatRelative } from "@/lib/utils";

interface Draft {
  hostname: string;
  acme_contact: string;
  acme_directory_url: string;
  http_redirect_enabled: boolean;
  tls_handshake_timeout_secs: string;
  backup_recovery_recipient: string;
  backup_repository_url: string;
  backup_interval_minutes: string;
  backup_local_retention: string;
  minimum_free_gib: string;
}

const EMPTY: Draft = {
  hostname: "",
  acme_contact: "",
  acme_directory_url: "",
  http_redirect_enabled: true,
  tls_handshake_timeout_secs: "10",
  backup_recovery_recipient: "",
  backup_repository_url: "",
  backup_interval_minutes: "1440",
  backup_local_retention: "2",
  minimum_free_gib: "2",
};

export default function AdminEdgeRoute() {
  const query = useEdgeAdmin();
  const update = useUpdateEdgeAdmin();
  const [draft, setDraft] = useState<Draft>(EMPTY);
  const loadedPending = useRef("");

  useEffect(() => {
    if (!query.data) return;
    const signature = JSON.stringify(query.data.pending);
    if (signature !== loadedPending.current) {
      loadedPending.current = signature;
      setDraft(toDraft(query.data.pending));
    }
  }, [query.data]);

  if (query.error) {
    return (
      <AdminPage
        title="Edge runtime"
        description="Production listener, recovery, storage, and resource policy."
      >
        <SurfaceState
          variant="error"
          title="Edge runtime management unavailable"
          description={(query.error as Error).message}
        />
      </AdminPage>
    );
  }

  if (!query.data) {
    return (
      <AdminPage
        title="Edge runtime"
        description="Production listener, recovery, storage, and resource policy."
      >
        <Skeleton className="h-40 rounded-xl" />
        <Skeleton className="h-80 rounded-xl" />
      </AdminPage>
    );
  }

  const snapshot = query.data;
  const health = snapshot.health;
  return (
    <AdminPage
      title="Edge runtime"
      description="Manage the durable configuration for this self-contained production server. Changes are validated and stored inside RocksDB, then applied together on restart."
    >
      {snapshot.restart_required && (
        <div className="flex items-start gap-3 rounded-xl border border-warning/30 bg-warning/5 p-4">
          <RefreshCcw className="mt-0.5 size-4 shrink-0 text-warning" />
          <div>
            <p className="text-sm font-medium">Restart required</p>
            <p className="mt-1 text-xs leading-5 text-muted-foreground">
              Pending settings are durable but the running TLS listener,
              scheduler, object-store client, retention policy, and disk reserve
              still use the active values shown before your edit.
            </p>
          </div>
        </div>
      )}

      <div className="grid gap-3 sm:grid-cols-3">
        <HealthCard
          icon={<ShieldCheck />}
          label="TLS certificate"
          value={health.tls.ready ? "Ready" : "Not ready"}
          detail={
            health.tls.certificate_not_after
              ? `Expires ${formatRelative(health.tls.certificate_not_after)}`
              : "Waiting for ACME issuance"
          }
          healthy={health.tls.ready}
        />
        <HealthCard
          icon={<Cloud />}
          label="ACME lifecycle"
          value={health.acme.last_success_at ? "Active" : "Starting"}
          detail={
            health.acme.last_failure_at
              ? `Last failure ${formatRelative(health.acme.last_failure_at)}`
              : health.acme.last_success_at
                ? `Last event ${formatRelative(health.acme.last_success_at)}`
                : "No lifecycle event observed yet"
          }
          healthy={
            !health.acme.last_failure_at || Boolean(health.acme.last_success_at)
          }
        />
        <HealthCard
          icon={<HardDrive />}
          label="Managed disk"
          value={health.disk?.ready ? "Healthy" : "Below reserve"}
          detail={
            health.disk?.available_bytes == null
              ? health.disk?.error || "Disk probe unavailable"
              : `${formatBytes(health.disk.available_bytes)} available · ${formatBytes(
                  health.disk.minimum_free_bytes
                )} reserved`
          }
          healthy={Boolean(health.disk?.ready)}
        />
      </div>

      <AdminSection
        title="Active appliance"
        description="Identity of the generation currently owned by this process. Storage paths are fixed at process start."
      >
        <div className="grid gap-4 text-xs sm:grid-cols-3">
          <RuntimeValue label="Profile" value={snapshot.runtime.profile} />
          <RuntimeValue
            label="Layout"
            value={`v${snapshot.runtime.layout_version}`}
          />
          <RuntimeValue
            label="Generation"
            value={snapshot.runtime.generation}
            mono
          />
          <RuntimeValue
            label="Data directory"
            value={snapshot.runtime.data_dir}
            mono
            wide
          />
        </div>
      </AdminSection>

      <form
        className="space-y-6"
        onSubmit={(event) => {
          event.preventDefault();
          update.mutate({
            hostname: draft.hostname,
            acme_contact: draft.acme_contact,
            acme_directory_url: draft.acme_directory_url,
            http_redirect_enabled: draft.http_redirect_enabled,
            tls_handshake_timeout_secs: Number(
              draft.tls_handshake_timeout_secs
            ),
            backup_recovery_recipient: draft.backup_recovery_recipient,
            backup_repository_url: draft.backup_repository_url,
            backup_interval_secs: Number(draft.backup_interval_minutes) * 60,
            backup_local_retention: Number(draft.backup_local_retention),
            minimum_free_bytes: Math.round(
              Number(draft.minimum_free_gib) * 2 ** 30
            ),
          });
        }}
      >
        <AdminSection
          title="HTTPS and ACME"
          description="sbol-db terminates TLS itself. ACME account and certificate keys remain private on disk and are included in complete backups."
        >
          <div className="grid gap-5 md:grid-cols-2">
            <Field
              label="Public hostname"
              hint="One concrete DNS name; wildcards and IP addresses are rejected."
            >
              <Input
                value={draft.hostname}
                onChange={(event) =>
                  setDraftValue(setDraft, "hostname", event.target.value)
                }
                placeholder="registry.example.org"
                required
              />
            </Field>
            <Field
              label="ACME contact email"
              hint="Used by the certificate authority for account and expiry notices."
            >
              <Input
                type="email"
                value={draft.acme_contact}
                onChange={(event) =>
                  setDraftValue(setDraft, "acme_contact", event.target.value)
                }
                required
              />
            </Field>
            <Field label="ACME directory URL" className="md:col-span-2">
              <Input
                value={draft.acme_directory_url}
                onChange={(event) =>
                  setDraftValue(
                    setDraft,
                    "acme_directory_url",
                    event.target.value
                  )
                }
                inputMode="url"
                required
              />
            </Field>
            <Field label="TLS handshake timeout (seconds)">
              <Input
                type="number"
                min={1}
                max={60}
                value={draft.tls_handshake_timeout_secs}
                onChange={(event) =>
                  setDraftValue(
                    setDraft,
                    "tls_handshake_timeout_secs",
                    event.target.value
                  )
                }
                required
              />
            </Field>
            <Toggle
              title="Redirect HTTP to HTTPS"
              description="Keep port 80 available only for canonical redirects to this hostname."
              checked={draft.http_redirect_enabled}
              onChange={(checked) =>
                setDraft((value) => ({
                  ...value,
                  http_redirect_enabled: checked,
                }))
              }
            />
          </div>
        </AdminSection>

        <AdminSection
          title="Complete backup policy"
          description="The repository URL contains no credentials. S3/GCS authentication comes from the server's workload identity or provider environment."
        >
          <div className="grid gap-5 md:grid-cols-2">
            <Field
              label="Object-store repository"
              hint="Use s3://bucket/instance-prefix or gs://bucket/instance-prefix."
              className="md:col-span-2"
            >
              <Input
                value={draft.backup_repository_url}
                onChange={(event) =>
                  setDraftValue(
                    setDraft,
                    "backup_repository_url",
                    event.target.value
                  )
                }
                placeholder="s3://backups/registry/production"
                className="font-mono"
                required
              />
            </Field>
            <Field
              label="Recovery recipient"
              hint="Public age X25519 recipient. Its private identity must remain off-server."
              className="md:col-span-2"
            >
              <Input
                value={draft.backup_recovery_recipient}
                onChange={(event) =>
                  setDraftValue(
                    setDraft,
                    "backup_recovery_recipient",
                    event.target.value
                  )
                }
                className="font-mono text-xs"
                required
              />
            </Field>
            <Field
              label="Backup cadence (minutes)"
              hint="15 minutes to 30 days."
            >
              <Input
                type="number"
                min={15}
                max={43_200}
                step={15}
                value={draft.backup_interval_minutes}
                onChange={(event) =>
                  setDraftValue(
                    setDraft,
                    "backup_interval_minutes",
                    event.target.value
                  )
                }
                required
              />
            </Field>
            <Field
              label="Verified artifacts retained locally"
              hint="Between 1 and 30. Remote objects are not pruned."
            >
              <Input
                type="number"
                min={1}
                max={30}
                value={draft.backup_local_retention}
                onChange={(event) =>
                  setDraftValue(
                    setDraft,
                    "backup_local_retention",
                    event.target.value
                  )
                }
                required
              />
            </Field>
            <Field
              label="Free-space reserve (GiB)"
              hint="Readiness fails below this reserve; backup preflight protects it."
            >
              <Input
                type="number"
                min={0.25}
                step={0.25}
                value={draft.minimum_free_gib}
                onChange={(event) =>
                  setDraftValue(
                    setDraft,
                    "minimum_free_gib",
                    event.target.value
                  )
                }
                required
              />
            </Field>
            <div className="flex items-start gap-3 rounded-lg border bg-muted/30 p-4">
              <KeyRound className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
              <p className="text-xs leading-5 text-muted-foreground">
                Secret cloud credentials and the recovery identity are
                deliberately not accepted or stored by this page.
              </p>
            </div>
          </div>
        </AdminSection>

        <div className="flex flex-wrap items-center justify-between gap-4">
          <MutationStatus
            pending={update.isPending}
            error={update.error}
            success={
              update.isSuccess
                ? update.data.restart_required
                  ? "Settings saved. Restart sbol-db to apply them."
                  : "Settings match the active runtime."
                : null
            }
          />
          <div className="flex gap-2">
            <Button
              type="button"
              variant="outline"
              disabled={update.isPending}
              onClick={() => setDraft(toDraft(snapshot.active))}
            >
              <RefreshCcw /> Revert to active
            </Button>
            <Button
              type="submit"
              disabled={update.isPending || !draft.hostname.trim()}
            >
              <Save /> Save pending settings
            </Button>
          </div>
        </div>
      </form>
    </AdminPage>
  );
}

function toDraft(settings: EdgeSettings): Draft {
  return {
    hostname: settings.hostname,
    acme_contact: settings.acme_contact,
    acme_directory_url: settings.acme_directory_url,
    http_redirect_enabled: settings.http_redirect_enabled,
    tls_handshake_timeout_secs: String(settings.tls_handshake_timeout_secs),
    backup_recovery_recipient: settings.backup_recovery_recipient,
    backup_repository_url: settings.backup_repository_url,
    backup_interval_minutes: String(settings.backup_interval_secs / 60),
    backup_local_retention: String(settings.backup_local_retention),
    minimum_free_gib: String(settings.minimum_free_bytes / 2 ** 30),
  };
}

function setDraftValue<K extends keyof Draft>(
  setter: React.Dispatch<React.SetStateAction<Draft>>,
  key: K,
  value: Draft[K]
) {
  setter((draft) => ({ ...draft, [key]: value }));
}

function HealthCard({
  icon,
  label,
  value,
  detail,
  healthy,
}: {
  icon: React.ReactNode;
  label: string;
  value: string;
  detail: string;
  healthy: boolean;
}) {
  return (
    <div className="rounded-xl border bg-card p-4 shadow-sm">
      <div className="flex items-start justify-between gap-3">
        <span className={healthy ? "text-success" : "text-warning"}>
          {icon}
        </span>
        <Badge variant="outline">{healthy ? "healthy" : "attention"}</Badge>
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

function RuntimeValue({
  label,
  value,
  mono,
  wide,
}: {
  label: string;
  value: string;
  mono?: boolean;
  wide?: boolean;
}) {
  return (
    <div className={wide ? "sm:col-span-3" : undefined}>
      <p className="text-[11px] uppercase tracking-wide text-muted-foreground">
        {label}
      </p>
      <p
        className={`mt-1 break-all text-sm ${mono ? "font-mono" : "font-medium"}`}
      >
        {value}
      </p>
    </div>
  );
}

function Toggle({
  title,
  description,
  checked,
  onChange,
}: {
  title: string;
  description: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <label className="flex cursor-pointer items-center justify-between gap-5 rounded-lg border px-4 py-3">
      <span>
        <span className="block text-sm font-medium">{title}</span>
        <span className="mt-1 block text-[11px] leading-4 text-muted-foreground">
          {description}
        </span>
      </span>
      <input
        type="checkbox"
        checked={checked}
        onChange={(event) => onChange(event.target.checked)}
        className="size-4 shrink-0 accent-primary"
      />
    </label>
  );
}
