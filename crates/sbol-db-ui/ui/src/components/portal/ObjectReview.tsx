import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  CheckCircle2,
  ClipboardCheck,
  History,
  Loader2,
  MessageSquareWarning,
  Send,
} from "lucide-react";
import { useState } from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import {
  decideObjectReview,
  requestObjectReview,
  type PortalObjectDetails,
  type ReviewCase,
  type ReviewStatus,
} from "@/features/portal/api";
import {
  portalKeys,
  useObjectReview,
  useSession,
} from "@/features/portal/queries";
import { cn } from "@/lib/utils";

export function ObjectReview({ object }: { object: PortalObjectDetails }) {
  const session = useSession();
  const user = session.data?.user;
  const owns = user ? object.owners.includes(user.graph_uri) : false;
  const canManage = Boolean(user && (owns || user.is_admin));
  const canParticipate = Boolean(user?.is_curator);
  const enabled =
    object.visibility === "restricted" &&
    Boolean(user) &&
    (canManage || canParticipate);
  const review = useObjectReview(object.iri, enabled);

  if (!enabled) return null;
  if (review.isLoading) {
    return (
      <Card aria-label="Loading review status">
        <CardContent className="p-5 text-xs text-muted-foreground">
          Loading review status…
        </CardContent>
      </Card>
    );
  }
  // A curator may be able to read the object through a normal share without
  // being assigned its review. Keep that unrelated management state private.
  if (review.error && !canManage) return null;

  return (
    <ReviewCard
      object={object}
      review={review.data ?? null}
      canRequest={canManage}
      canDecide={Boolean(
        user &&
        review.data?.status === "pending" &&
        (user.is_admin || review.data.curator_graph === user.graph_uri)
      )}
      error={review.error}
    />
  );
}

function ReviewCard({
  object,
  review,
  canRequest,
  canDecide,
  error,
}: {
  object: PortalObjectDetails;
  review: ReviewCase | null;
  canRequest: boolean;
  canDecide: boolean;
  error: Error | null;
}) {
  const queryClient = useQueryClient();
  const [requestOpen, setRequestOpen] = useState(false);
  const [curator, setCurator] = useState("");
  const [requestNote, setRequestNote] = useState("");
  const [decisionNote, setDecisionNote] = useState("");
  const refresh = async () => {
    await Promise.all([
      queryClient.invalidateQueries({
        queryKey: portalKeys.objectReview(object.iri),
      }),
      queryClient.invalidateQueries({ queryKey: portalKeys.reviews }),
      queryClient.invalidateQueries({
        queryKey: portalKeys.objectActivity(object.iri),
      }),
      queryClient.invalidateQueries({
        queryKey: portalKeys.collaborators(object.iri),
      }),
      queryClient.invalidateQueries({ queryKey: portalKeys.shared }),
    ]);
  };
  const requestReview = useMutation({
    mutationFn: () =>
      requestObjectReview(
        object.iri,
        curator.trim(),
        requestNote.trim() || undefined
      ),
    onSuccess: async () => {
      setRequestOpen(false);
      setCurator("");
      setRequestNote("");
      await refresh();
    },
  });
  const decide = useMutation({
    mutationFn: (decision: "approve" | "request_changes") =>
      decideObjectReview(
        object.iri,
        decision,
        decisionNote.trim() || undefined
      ),
    onSuccess: async () => {
      setDecisionNote("");
      await refresh();
    },
  });

  return (
    <Card>
      <CardHeader className="border-b bg-muted/15 p-5">
        <div className="flex items-start justify-between gap-3">
          <div>
            <CardTitle className="flex items-center gap-2 text-base">
              <ClipboardCheck
                className="size-4 text-primary"
                aria-hidden="true"
              />
              Curator review
            </CardTitle>
            <p className="mt-1 text-xs leading-5 text-muted-foreground">
              A role-scoped review with an append-only decision history.
            </p>
          </div>
          {review && <ReviewBadge status={review.status} />}
        </div>
      </CardHeader>
      <CardContent className="space-y-4 p-5">
        {error && <MutationError error={error} />}
        {review ? (
          <ReviewSummary review={review} />
        ) : (
          <p className="text-xs leading-5 text-muted-foreground">
            No review has been requested for this object.
          </p>
        )}

        {canDecide && (
          <div className="space-y-3 border-t pt-4">
            <div className="space-y-2">
              <Label htmlFor="review-decision-note">Decision note</Label>
              <Textarea
                id="review-decision-note"
                value={decisionNote}
                maxLength={4000}
                onChange={(event) => setDecisionNote(event.target.value)}
                placeholder="Explain the decision for the submitter."
              />
            </div>
            <div className="grid grid-cols-2 gap-2">
              <Button
                type="button"
                size="sm"
                disabled={decide.isPending}
                onClick={() => decide.mutate("approve")}
              >
                {decide.isPending ? (
                  <Loader2 className="animate-spin motion-reduce:animate-none" />
                ) : (
                  <CheckCircle2 />
                )}
                Approve
              </Button>
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={decide.isPending}
                onClick={() => decide.mutate("request_changes")}
              >
                <MessageSquareWarning /> Changes
              </Button>
            </div>
            <MutationError error={decide.error} />
          </div>
        )}

        {canRequest && review?.status !== "pending" && (
          <div className="border-t pt-4">
            {!requestOpen ? (
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => setRequestOpen(true)}
              >
                <Send /> {review ? "Request another review" : "Request review"}
              </Button>
            ) : (
              <form
                className="space-y-3"
                onSubmit={(event) => {
                  event.preventDefault();
                  if (curator.trim()) requestReview.mutate();
                }}
              >
                <div className="space-y-2">
                  <Label htmlFor="review-curator">
                    Curator username or email
                  </Label>
                  <Input
                    id="review-curator"
                    value={curator}
                    onChange={(event) => setCurator(event.target.value)}
                    autoComplete="off"
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="review-request-note">Review brief</Label>
                  <Textarea
                    id="review-request-note"
                    value={requestNote}
                    maxLength={4000}
                    onChange={(event) => setRequestNote(event.target.value)}
                    placeholder="What should the curator verify?"
                  />
                </div>
                <div className="flex gap-2">
                  <Button
                    type="submit"
                    size="sm"
                    disabled={!curator.trim() || requestReview.isPending}
                  >
                    {requestReview.isPending ? (
                      <Loader2 className="animate-spin motion-reduce:animate-none" />
                    ) : (
                      <Send />
                    )}
                    Send request
                  </Button>
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onClick={() => setRequestOpen(false)}
                  >
                    Cancel
                  </Button>
                </div>
                <MutationError error={requestReview.error} />
              </form>
            )}
          </div>
        )}
      </CardContent>
    </Card>
  );
}

function ReviewSummary({ review }: { review: ReviewCase }) {
  return (
    <div className="space-y-3 text-xs">
      <dl className="space-y-2">
        <div className="flex justify-between gap-3">
          <dt className="text-muted-foreground">Assigned curator</dt>
          <dd className="truncate font-medium" title={review.curator_graph}>
            {accountLabel(review.curator_graph)}
          </dd>
        </div>
        <div className="flex justify-between gap-3">
          <dt className="text-muted-foreground">Last updated</dt>
          <dd className="font-medium">{formatTimestamp(review.updated_at)}</dd>
        </div>
      </dl>
      {review.note && (
        <p className="rounded-lg bg-muted/50 p-3 leading-5">{review.note}</p>
      )}
      <details>
        <summary className="flex min-h-11 cursor-pointer items-center gap-2 font-medium text-muted-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring">
          <History className="size-3.5" aria-hidden="true" />
          {review.events.length} audit event
          {review.events.length === 1 ? "" : "s"}
        </summary>
        <ol className="space-y-2 border-l pl-3">
          {review.events.map((event) => (
            <li key={event.iri}>
              <p className="font-medium">{actionLabel(event.action)}</p>
              <p className="mt-0.5 text-muted-foreground">
                {formatTimestamp(event.occurred_at)} ·{" "}
                {accountLabel(event.actor_graph)}
              </p>
            </li>
          ))}
        </ol>
      </details>
    </div>
  );
}

function ReviewBadge({ status }: { status: ReviewStatus }) {
  const label =
    status === "pending"
      ? "Pending"
      : status === "approved"
        ? "Approved"
        : "Changes requested";
  return (
    <Badge
      variant="outline"
      className={cn(
        "shrink-0",
        status === "approved" && "border-success/30 bg-success/10 text-success",
        status === "changes_requested" &&
          "border-warning/30 bg-warning/10 text-warning-foreground"
      )}
    >
      {label}
    </Badge>
  );
}

function accountLabel(graph: string) {
  const value = graph.split("/").filter(Boolean).at(-1);
  return value ? `@${decodeURIComponent(value)}` : graph;
}

function actionLabel(action: string) {
  return action
    .replaceAll("_", " ")
    .replace(/^./, (letter) => letter.toUpperCase());
}

function formatTimestamp(value: string) {
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(value));
}

function MutationError({ error }: { error: Error | null }) {
  if (!error) return null;
  return (
    <p className="text-xs leading-5 text-destructive" role="alert">
      {error.message}
    </p>
  );
}
