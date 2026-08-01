import { Download, FileCheck2, RotateCcw, ShieldAlert } from "lucide-react";
import { useState } from "react";

import {
  AdminPage,
  AdminSection,
  MutationStatus,
} from "@/components/admin/AdminPage";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { downloadBackup, type BackupArchive } from "@/features/admin/api";
import { useRestoreBackup, useValidateBackup } from "@/features/admin/queries";

export default function AdminBackupRoute() {
  const validate = useValidateBackup();
  const restore = useRestoreBackup();
  const [archive, setArchive] = useState<BackupArchive | null>(null);
  const [confirmation, setConfirmation] = useState("");
  const [fileError, setFileError] = useState<Error | null>(null);
  const [downloading, setDownloading] = useState(false);
  const [downloadError, setDownloadError] = useState<Error | null>(null);

  return (
    <AdminPage
      title="Backup and restore"
      description="Export a portable, integrity-checked archive of SBOL document graphs, then validate an archive before any restore mutation is enabled."
    >
      <AdminSection
        title="Export registry graphs"
        description="The archive includes document RDF and graph metadata. Accounts, password material, tokens, deployment configuration, remote secrets, and attachment blobs are deliberately excluded."
        action={
          <Button
            disabled={downloading}
            onClick={async () => {
              setDownloading(true);
              setDownloadError(null);
              try {
                await downloadBackup();
              } catch (error) {
                setDownloadError(
                  error instanceof Error ? error : new Error("Download failed")
                );
              } finally {
                setDownloading(false);
              }
            }}
          >
            <Download /> {downloading ? "Preparing…" : "Download archive"}
          </Button>
        }
      >
        <div className="grid gap-3 sm:grid-cols-3">
          <ScopeItem label="Included" value="SBOL graph RDF" />
          <ScopeItem label="Integrity" value="SHA3-256 checksum" />
          <ScopeItem label="Restore" value="Atomic document batch" />
        </div>
        <div className="mt-4">
          <MutationStatus pending={downloading} error={downloadError} />
        </div>
      </AdminSection>

      <AdminSection
        title="Validate archive"
        description="Validation checks the format version, checksum, document IRIs, and every N-Triples body without changing the registry."
      >
        <Input
          type="file"
          accept="application/json,.json"
          onChange={async (event) => {
            const file = event.target.files?.[0];
            setArchive(null);
            setConfirmation("");
            validate.reset();
            restore.reset();
            setFileError(null);
            if (!file) return;
            try {
              const parsed = JSON.parse(await file.text()) as BackupArchive;
              setArchive(parsed);
              validate.mutate(parsed);
            } catch (error) {
              setFileError(
                error instanceof Error
                  ? error
                  : new Error("Invalid archive JSON")
              );
            }
          }}
        />
        <div className="mt-4">
          <MutationStatus
            pending={validate.isPending}
            error={fileError || validate.error}
          />
        </div>
        {validate.data && (
          <div className="mt-5 rounded-lg border border-success/25 bg-success/5 p-4">
            <div className="flex flex-wrap items-center gap-2">
              <FileCheck2 className="size-4 text-success" />
              <p className="text-sm font-medium">Archive integrity verified</p>
              <Badge variant="outline">v{validate.data.version}</Badge>
              <Badge variant="outline">
                {validate.data.documents} document
                {validate.data.documents === 1 ? "" : "s"}
              </Badge>
            </div>
            <code className="mt-3 block break-all text-[11px] text-muted-foreground">
              {validate.data.checksum}
            </code>
            <p className="mt-3 text-xs leading-5 text-muted-foreground">
              Excluded: {validate.data.excludes.join(", ")}.
            </p>
          </div>
        )}
      </AdminSection>

      <AdminSection
        title="Restore verified archive"
        description="Documents with logical document IRIs replace matching documents atomically; documents without one must not collide with existing content. A search rebuild is queued after commit."
      >
        <div className="rounded-lg border border-warning/30 bg-warning/5 p-4">
          <div className="flex items-start gap-3">
            <ShieldAlert className="mt-0.5 size-4 shrink-0 text-warning" />
            <div className="min-w-0 flex-1">
              <p className="text-sm font-medium">
                Explicit confirmation required
              </p>
              <p className="mt-1 text-xs leading-5 text-muted-foreground">
                {validate.data ? (
                  <>
                    Type{" "}
                    <code className="font-semibold text-foreground">
                      {validate.data.confirmation}
                    </code>{" "}
                    to enable restore.
                  </>
                ) : (
                  "Select and validate an archive first."
                )}
              </p>
              <div className="mt-3 flex flex-wrap gap-2">
                <Input
                  value={confirmation}
                  onChange={(event) => setConfirmation(event.target.value)}
                  disabled={!validate.data}
                  className="max-w-md bg-background font-mono"
                  aria-label="Restore confirmation"
                />
                <Button
                  variant="destructive"
                  disabled={
                    !archive ||
                    !validate.data ||
                    restore.isPending ||
                    confirmation !== validate.data.confirmation
                  }
                  onClick={() => {
                    if (archive) restore.mutate({ archive, confirmation });
                  }}
                >
                  <RotateCcw /> Restore archive
                </Button>
              </div>
            </div>
          </div>
        </div>
        <div className="mt-4">
          <MutationStatus
            pending={restore.isPending}
            error={restore.error}
            success={
              restore.data
                ? `${restore.data.documents} documents restored; rebuild job ${restore.data.rebuild_job.id.slice(0, 8)} queued.`
                : null
            }
          />
        </div>
      </AdminSection>
    </AdminPage>
  );
}

function ScopeItem({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-lg border bg-background px-4 py-3">
      <p className="text-[11px] uppercase tracking-wide text-muted-foreground">
        {label}
      </p>
      <p className="mt-1 text-sm font-medium">{value}</p>
    </div>
  );
}
