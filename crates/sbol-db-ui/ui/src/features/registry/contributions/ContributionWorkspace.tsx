import { useEffect, useMemo, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  ArrowRight,
  Check,
  CircleAlert,
  FileCheck2,
  FileUp,
  FolderKanban,
  Loader2,
  RotateCcw,
  TriangleAlert,
} from "lucide-react";
import { Link } from "react-router-dom";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/native-select";
import { Textarea } from "@/components/ui/textarea";
import {
  createContribution,
  type ContributionFormat,
  type ContributionOverwrite,
  type ContributionPreview,
  type ContributionRequest,
  validateContribution,
} from "@/features/registry/contributions/api";
import { discoveryKeys } from "@/features/registry/discovery/queries";
import { registryObjectKeys } from "@/features/registry/objects/queries";
import { shortIri } from "@/features/registry/objects/format";
import { publicObjectPath } from "@/lib/routes";

type Draft = {
  id: string;
  version: string;
  name: string;
  description: string;
  citations: string;
  format: ContributionFormat;
  overwrite: ContributionOverwrite;
  content: string;
  filename: string;
};

const initialDraft: Draft = {
  id: "",
  version: "1",
  name: "",
  description: "",
  citations: "",
  format: "rdfxml",
  overwrite: "fail",
  content: "",
  filename: "",
};

const formatOptions: Array<{
  value: ContributionFormat;
  label: string;
  description: string;
}> = [
  { value: "rdfxml", label: "SBOL RDF/XML", description: "SBOL 2 or SBOL 3" },
  { value: "turtle", label: "SBOL Turtle", description: "SBOL 2 or SBOL 3" },
  { value: "jsonld", label: "SBOL JSON-LD", description: "SBOL 3 RDF" },
  {
    value: "ntriples",
    label: "SBOL N-Triples",
    description: "SBOL 2 or SBOL 3",
  },
  { value: "genbank", label: "GenBank", description: "Converted to SBOL 3" },
  { value: "fasta", label: "FASTA", description: "Converted to SBOL 3" },
];

export function ContributionWorkspace() {
  const queryClient = useQueryClient();
  const [draft, setDraft] = useState<Draft>(initialDraft);
  const [preview, setPreview] = useState<ContributionPreview | null>(null);
  const [fileError, setFileError] = useState<string | null>(null);

  const validation = useMutation({
    mutationFn: validateContribution,
    onSuccess: setPreview,
  });
  const commit = useMutation({
    mutationFn: createContribution,
    onSuccess: (created) => {
      queryClient.invalidateQueries({ queryKey: discoveryKeys.searches() });
      queryClient.invalidateQueries({
        queryKey: registryObjectKeys.normalized(created.collection_uri),
      });
    },
  });

  const request = useMemo(() => contributionRequest(draft), [draft]);
  const formReady =
    draft.id.trim().length > 0 &&
    draft.version.trim().length > 0 &&
    draft.content.trim().length > 0;

  useEffect(() => {
    setPreview(null);
    validation.reset();
    commit.reset();
    // The serialized request is the review boundary: any changed input makes
    // the previous identity/consequence preview stale.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [JSON.stringify(request)]);

  const reset = () => {
    setDraft(initialDraft);
    setPreview(null);
    setFileError(null);
    validation.reset();
    commit.reset();
  };

  if (commit.data) {
    return (
      <div className="mx-auto max-w-4xl px-4 py-14 sm:px-6 lg:px-8">
        <Card className="overflow-hidden border-primary/25">
          <div className="h-1 bg-primary" />
          <CardContent className="flex flex-col items-start gap-5 p-7 sm:p-10">
            <span className="flex size-12 items-center justify-center rounded-2xl bg-primary/10 text-primary">
              <Check className="size-6" />
            </span>
            <div>
              <p className="text-xs font-semibold uppercase tracking-[0.14em] text-primary">
                Contribution committed
              </p>
              <h1 className="mt-2 text-3xl font-semibold tracking-tight">
                Your collection is ready
              </h1>
              <p className="mt-3 max-w-2xl text-sm leading-6 text-muted-foreground">
                SBOL DB wrote {commit.data.triple_count.toLocaleString()}{" "}
                triples and minted {commit.data.members.length.toLocaleString()}{" "}
                member
                {commit.data.members.length === 1 ? "" : "s"}. The collection is
                private until you publish it.
              </p>
            </div>
            <code className="w-full break-all rounded-xl border bg-muted/30 p-4 font-mono text-xs">
              {commit.data.collection_uri}
            </code>
            <div className="flex flex-wrap gap-3">
              <Button asChild>
                <Link to={publicObjectPath(commit.data.collection_uri)}>
                  Inspect collection <ArrowRight />
                </Link>
              </Button>
              <Button variant="outline" onClick={reset}>
                <FileUp /> Contribute another
              </Button>
              <Button asChild variant="ghost">
                <Link to="/workspace">Open workspace</Link>
              </Button>
            </div>
          </CardContent>
        </Card>
      </div>
    );
  }

  return (
    <div className="mx-auto max-w-7xl px-4 py-10 sm:px-6 lg:px-8">
      <header className="max-w-3xl">
        <div className="flex items-center gap-2 text-xs font-semibold uppercase tracking-[0.14em] text-primary">
          <FolderKanban className="size-3.5" aria-hidden="true" /> Account
          workspace
        </div>
        <h1 className="mt-3 text-3xl font-semibold tracking-tight sm:text-4xl">
          Contribute a design collection
        </h1>
        <p className="mt-4 text-base leading-7 text-muted-foreground">
          Validate first, review the exact identities and persistence
          consequences, then commit. Preview and cancellation never write data.
        </p>
      </header>

      <WorkflowSteps hasPreview={preview !== null} />

      <div className="mt-8 grid items-start gap-6 lg:grid-cols-[minmax(0,1fr)_23rem]">
        <Card>
          <CardHeader>
            <CardTitle>Contribution source</CardTitle>
            <CardDescription>
              SBOL 2 and SBOL 3 are preserved. GenBank and FASTA are converted
              to SBOL 3 before identity minting.
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-6">
            <div className="grid gap-5 sm:grid-cols-2">
              <Field label="Collection ID" htmlFor="contribution-id">
                <Input
                  id="contribution-id"
                  autoComplete="off"
                  value={draft.id}
                  onChange={(event) =>
                    setDraft((current) => ({
                      ...current,
                      id: event.target.value,
                    }))
                  }
                  placeholder="my_designs"
                  aria-describedby="contribution-id-help"
                />
                <p
                  id="contribution-id-help"
                  className="text-xs text-muted-foreground"
                >
                  Becomes part of the permanent collection IRI.
                </p>
              </Field>
              <Field label="Version" htmlFor="contribution-version">
                <Input
                  id="contribution-version"
                  autoComplete="off"
                  value={draft.version}
                  onChange={(event) =>
                    setDraft((current) => ({
                      ...current,
                      version: event.target.value,
                    }))
                  }
                  placeholder="1"
                />
              </Field>
            </div>

            <div className="grid gap-5 sm:grid-cols-2">
              <Field label="Collection name" htmlFor="contribution-name">
                <Input
                  id="contribution-name"
                  value={draft.name}
                  onChange={(event) =>
                    setDraft((current) => ({
                      ...current,
                      name: event.target.value,
                    }))
                  }
                  placeholder="Optional human-readable title"
                />
              </Field>
              <Field label="PubMed IDs" htmlFor="contribution-citations">
                <Input
                  id="contribution-citations"
                  value={draft.citations}
                  onChange={(event) =>
                    setDraft((current) => ({
                      ...current,
                      citations: event.target.value,
                    }))
                  }
                  placeholder="12345678, 23456789"
                />
              </Field>
            </div>

            <Field label="Description" htmlFor="contribution-description">
              <Textarea
                id="contribution-description"
                value={draft.description}
                onChange={(event) =>
                  setDraft((current) => ({
                    ...current,
                    description: event.target.value,
                  }))
                }
                placeholder="What does this collection contain?"
              />
            </Field>

            <div className="grid gap-5 sm:grid-cols-2">
              <Field label="Input format" htmlFor="contribution-format">
                <NativeSelect
                  id="contribution-format"
                  value={draft.format}
                  onChange={(event) =>
                    setDraft((current) => ({
                      ...current,
                      format: event.target.value as ContributionFormat,
                    }))
                  }
                >
                  {formatOptions.map((option) => (
                    <option key={option.value} value={option.value}>
                      {option.label} — {option.description}
                    </option>
                  ))}
                </NativeSelect>
              </Field>
              <Field
                label="If identity exists"
                htmlFor="contribution-overwrite"
              >
                <NativeSelect
                  id="contribution-overwrite"
                  value={draft.overwrite}
                  onChange={(event) =>
                    setDraft((current) => ({
                      ...current,
                      overwrite: event.target.value as ContributionOverwrite,
                    }))
                  }
                >
                  <option value="fail">Stop and report the conflict</option>
                  <option value="replace">Replace that exact submission</option>
                  <option value="merge">
                    Merge into that exact submission
                  </option>
                </NativeSelect>
              </Field>
            </div>

            <Field label="Choose a file" htmlFor="contribution-file">
              <label
                htmlFor="contribution-file"
                className="flex min-h-24 cursor-pointer items-center gap-4 rounded-xl border border-dashed bg-muted/15 px-4 py-4 transition-colors hover:border-primary/45 hover:bg-primary/[0.03] focus-within:ring-2 focus-within:ring-ring focus-within:ring-offset-2"
              >
                <span className="flex size-10 shrink-0 items-center justify-center rounded-xl bg-background text-primary shadow-sm">
                  <FileUp className="size-5" />
                </span>
                <span className="min-w-0">
                  <span className="block truncate text-sm font-medium">
                    {draft.filename || "Select an SBOL, GenBank, or FASTA file"}
                  </span>
                  <span className="mt-1 block text-xs text-muted-foreground">
                    The file is read locally and sent only when you validate.
                  </span>
                </span>
                <input
                  id="contribution-file"
                  type="file"
                  className="sr-only"
                  accept=".xml,.rdf,.ttl,.jsonld,.nt,.gb,.gbk,.genbank,.fa,.fasta,.fna,.faa"
                  onChange={(event) => {
                    const file = event.target.files?.[0];
                    if (!file) return;
                    setFileError(null);
                    file
                      .text()
                      .then((content) =>
                        setDraft((current) => ({
                          ...current,
                          content,
                          filename: file.name,
                          format:
                            formatForFilename(file.name) ?? current.format,
                        }))
                      )
                      .catch(() =>
                        setFileError("The selected file could not be read.")
                      );
                  }}
                />
              </label>
              {fileError && <InlineError message={fileError} />}
            </Field>

            <Field label="Serialized content" htmlFor="contribution-content">
              <Textarea
                id="contribution-content"
                className="min-h-72 font-mono text-xs leading-5"
                spellCheck={false}
                value={draft.content}
                onChange={(event) =>
                  setDraft((current) => ({
                    ...current,
                    content: event.target.value,
                    filename: "",
                  }))
                }
                placeholder="Paste serialized SBOL, GenBank, or FASTA content…"
              />
            </Field>

            {(validation.error || commit.error) && (
              <InlineError
                message={
                  (validation.error || commit.error)?.message ||
                  "The request failed."
                }
              />
            )}

            <div className="flex flex-wrap justify-between gap-3 border-t pt-5">
              <Button type="button" variant="ghost" onClick={reset}>
                <RotateCcw /> Clear draft
              </Button>
              <Button
                type="button"
                disabled={
                  !formReady || validation.isPending || commit.isPending
                }
                onClick={() => validation.mutate(request)}
              >
                {validation.isPending ? (
                  <Loader2 className="animate-spin motion-reduce:animate-none" />
                ) : (
                  <FileCheck2 />
                )}
                {validation.isPending ? "Validating…" : "Validate contribution"}
              </Button>
            </div>
          </CardContent>
        </Card>

        <aside
          className="space-y-4 lg:sticky lg:top-24"
          aria-label="Contribution review"
        >
          <PreviewCard preview={preview} />
          <Card>
            <CardContent className="p-5">
              <Button
                type="button"
                className="w-full"
                disabled={
                  !preview ||
                  preview.consequence === "reject_conflict" ||
                  commit.isPending ||
                  validation.isPending
                }
                onClick={() => commit.mutate(request)}
              >
                {commit.isPending ? (
                  <Loader2 className="animate-spin motion-reduce:animate-none" />
                ) : (
                  <ArrowRight />
                )}
                {commit.isPending ? "Committing…" : "Commit collection"}
              </Button>
              <p className="mt-3 text-xs leading-5 text-muted-foreground">
                Commit is enabled only for the exact draft that passed preview.
                The server validates it again before writing.
              </p>
            </CardContent>
          </Card>
        </aside>
      </div>
    </div>
  );
}

function WorkflowSteps({ hasPreview }: { hasPreview: boolean }) {
  const steps = [
    { label: "Describe", state: "complete" },
    { label: "Validate", state: hasPreview ? "complete" : "current" },
    { label: "Commit", state: hasPreview ? "current" : "upcoming" },
  ] as const;
  return (
    <ol
      className="mt-7 flex max-w-xl items-center"
      aria-label="Contribution steps"
    >
      {steps.map((step, index) => (
        <li
          key={step.label}
          className="flex min-w-0 flex-1 items-center last:flex-none"
        >
          <span
            className="flex items-center gap-2"
            aria-current={step.state === "current" ? "step" : undefined}
          >
            <span
              className={
                step.state === "upcoming"
                  ? "flex size-7 items-center justify-center rounded-full border text-xs text-muted-foreground"
                  : "flex size-7 items-center justify-center rounded-full bg-primary text-xs font-semibold text-primary-foreground"
              }
            >
              {step.state === "complete" ? (
                <Check className="size-3.5" />
              ) : (
                index + 1
              )}
            </span>
            <span className="hidden text-xs font-medium sm:block">
              {step.label}
            </span>
          </span>
          {index < steps.length - 1 && (
            <span className="mx-3 h-px flex-1 bg-border" />
          )}
        </li>
      ))}
    </ol>
  );
}

function PreviewCard({ preview }: { preview: ContributionPreview | null }) {
  if (!preview) {
    return (
      <Card>
        <CardHeader>
          <CardTitle className="text-base">Validation preview</CardTitle>
          <CardDescription>
            No graph is written during this step.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <div className="rounded-xl border border-dashed p-5 text-sm leading-6 text-muted-foreground">
            Complete the source form and validate to see minted identities,
            conversions, member count, and conflicts.
          </div>
        </CardContent>
      </Card>
    );
  }

  const blocked = preview.consequence === "reject_conflict";
  return (
    <Card className={blocked ? "border-amber-500/35" : "border-primary/25"}>
      <CardHeader>
        <div className="flex items-start justify-between gap-3">
          <div>
            <CardTitle className="text-base">Validated draft</CardTitle>
            <CardDescription className="mt-1">
              {preview.triple_count.toLocaleString()} triples ·{" "}
              {preview.members.length.toLocaleString()} members
            </CardDescription>
          </div>
          <Badge variant={blocked ? "outline" : "secondary"}>
            {blocked ? "Conflict" : "Ready"}
          </Badge>
        </div>
      </CardHeader>
      <CardContent className="space-y-4">
        <dl className="grid gap-3 text-xs">
          <PreviewFact
            label="Input"
            value={preview.source_standard.toUpperCase()}
          />
          <PreviewFact
            label="Stored as"
            value={preview.normalized_standard.toUpperCase()}
          />
          <PreviewFact
            label="Action"
            value={consequenceLabel(preview.consequence)}
          />
        </dl>
        <div>
          <div className="text-xs font-medium text-muted-foreground">
            Collection IRI
          </div>
          <code className="mt-1.5 block break-all rounded-lg bg-muted/40 p-3 font-mono text-[10px] leading-4">
            {preview.collection_uri}
          </code>
        </div>
        {preview.members.length > 0 && (
          <div>
            <div className="text-xs font-medium text-muted-foreground">
              Minted members
            </div>
            <ul className="mt-2 space-y-1.5">
              {preview.members.slice(0, 5).map((member) => (
                <li key={member} className="truncate text-xs" title={member}>
                  {shortIri(member)}
                </li>
              ))}
            </ul>
            {preview.members.length > 5 && (
              <p className="mt-2 text-xs text-muted-foreground">
                +{preview.members.length - 5} more members
              </p>
            )}
          </div>
        )}
        {preview.notices.map((notice) => (
          <div
            key={`${notice.code}-${notice.message}`}
            className={
              notice.level === "warning"
                ? "flex gap-2 rounded-lg border border-amber-500/25 bg-amber-500/5 p-3 text-xs leading-5 text-amber-900 dark:text-amber-100"
                : "flex gap-2 rounded-lg border bg-muted/20 p-3 text-xs leading-5 text-muted-foreground"
            }
          >
            {notice.level === "warning" ? (
              <TriangleAlert className="mt-0.5 size-3.5 shrink-0" />
            ) : (
              <CircleAlert className="mt-0.5 size-3.5 shrink-0" />
            )}
            <span>{notice.message}</span>
          </div>
        ))}
      </CardContent>
    </Card>
  );
}

function Field({
  label,
  htmlFor,
  children,
}: {
  label: string;
  htmlFor: string;
  children: React.ReactNode;
}) {
  return (
    <div className="space-y-2">
      <Label htmlFor={htmlFor}>{label}</Label>
      {children}
    </div>
  );
}

function PreviewFact({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-center justify-between gap-4 border-b pb-2 last:border-0 last:pb-0">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="text-right font-medium">{value}</dd>
    </div>
  );
}

function InlineError({ message }: { message: string }) {
  return (
    <div
      role="alert"
      className="flex gap-2 rounded-lg border border-destructive/25 bg-destructive/5 p-3 text-sm text-destructive"
    >
      <CircleAlert className="mt-0.5 size-4 shrink-0" />
      <span>{message}</span>
    </div>
  );
}

function contributionRequest(draft: Draft): ContributionRequest {
  return {
    id: draft.id.trim(),
    version: draft.version.trim(),
    name: draft.name.trim() || undefined,
    description: draft.description.trim() || undefined,
    citations: draft.citations
      .split(",")
      .map((citation) => citation.trim())
      .filter(Boolean),
    format: draft.format,
    overwrite: draft.overwrite,
    content: draft.content,
  };
}

function formatForFilename(filename: string): ContributionFormat | null {
  const extension = filename.toLowerCase().split(".").pop();
  if (extension === "xml" || extension === "rdf") return "rdfxml";
  if (extension === "ttl") return "turtle";
  if (extension === "jsonld") return "jsonld";
  if (extension === "nt") return "ntriples";
  if (["gb", "gbk", "genbank"].includes(extension || "")) return "genbank";
  if (["fa", "fasta", "fna", "faa"].includes(extension || "")) return "fasta";
  return null;
}

function consequenceLabel(
  consequence: ContributionPreview["consequence"]
): string {
  switch (consequence) {
    case "create":
      return "Create a private collection";
    case "reject_conflict":
      return "Stop on identity conflict";
    case "replace":
      return "Replace existing collection";
    case "merge":
      return "Merge with existing collection";
  }
}
