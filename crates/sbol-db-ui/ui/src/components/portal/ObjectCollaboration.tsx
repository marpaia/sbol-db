import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  ArrowRightLeft,
  Check,
  Eye,
  Loader2,
  Trash2,
  UserPlus,
  UsersRound,
  X,
} from "lucide-react";
import { useState } from "react";
import { useNavigate } from "react-router-dom";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  grantObjectShare,
  revokeObjectShare,
  transferObjectOwnership,
  type PortalObjectDetails,
} from "@/features/portal/api";
import {
  portalKeys,
  useCollaborators,
  useSession,
} from "@/features/portal/queries";

export function ObjectCollaboration({
  object,
}: {
  object: PortalObjectDetails;
}) {
  const session = useSession();
  const user = session.data?.user;
  if (!user || object.visibility !== "restricted") return null;
  const owns = object.owners.includes(user.graph_uri);
  if (!owns && !user.is_admin) {
    return (
      <Card>
        <CardContent className="flex items-start gap-3 p-5">
          <span className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary">
            <Eye className="size-4" aria-hidden="true" />
          </span>
          <div>
            <p className="text-sm font-semibold">Shared with you</p>
            <p className="mt-1 text-xs leading-5 text-muted-foreground">
              You can inspect and download this private object. Editing,
              publishing, collaborator management, and deletion remain with its
              owner.
            </p>
          </div>
        </CardContent>
      </Card>
    );
  }
  return <CollaborationCard object={object} canTransfer={owns} />;
}

function CollaborationCard({
  object,
  canTransfer,
}: {
  object: PortalObjectDetails;
  canTransfer: boolean;
}) {
  const collaborators = useCollaborators(object.iri);
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const [recipient, setRecipient] = useState("");
  const [pendingRevoke, setPendingRevoke] = useState<string | null>(null);
  const [transferTarget, setTransferTarget] = useState("");
  const [transferConfirmation, setTransferConfirmation] = useState("");
  const [transferOpen, setTransferOpen] = useState(false);
  const refresh = async () => {
    await queryClient.invalidateQueries({
      queryKey: portalKeys.collaborators(object.iri),
    });
    await queryClient.invalidateQueries({ queryKey: portalKeys.shared });
  };
  const grant = useMutation({
    mutationFn: () => grantObjectShare(object.iri, recipient.trim()),
    onSuccess: async () => {
      setRecipient("");
      await refresh();
    },
  });
  const revoke = useMutation({
    mutationFn: (username: string) => revokeObjectShare(object.iri, username),
    onSuccess: async () => {
      setPendingRevoke(null);
      await refresh();
    },
  });
  const transfer = useMutation({
    mutationFn: () =>
      transferObjectOwnership(object.iri, transferTarget.trim()),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["portal"] });
      navigate("/workspace", { replace: true });
    },
  });

  return (
    <Card>
      <CardHeader className="border-b bg-muted/15 p-5">
        <CardTitle className="flex items-center gap-2 text-base">
          <UsersRound className="size-4 text-primary" aria-hidden="true" />
          Collaboration
        </CardTitle>
        <p className="text-xs leading-5 text-muted-foreground">
          Grant read-only access without changing ownership or publication
          status.
        </p>
      </CardHeader>
      <CardContent className="space-y-5 p-5">
        <form
          className="flex gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            if (recipient.trim()) grant.mutate();
          }}
        >
          <div className="min-w-0 flex-1 space-y-2">
            <Label htmlFor="share-recipient">Member username or email</Label>
            <Input
              id="share-recipient"
              value={recipient}
              onChange={(event) => setRecipient(event.target.value)}
              placeholder="collaborator"
              autoComplete="off"
            />
          </div>
          <Button
            type="submit"
            className="mt-7"
            disabled={!recipient.trim() || grant.isPending}
          >
            {grant.isPending ? (
              <Loader2 className="animate-spin motion-reduce:animate-none" />
            ) : (
              <UserPlus />
            )}
            Share
          </Button>
        </form>

        {collaborators.isLoading ? (
          <p className="text-xs text-muted-foreground">
            Loading collaborators…
          </p>
        ) : collaborators.error ? (
          <MutationError error={collaborators.error} />
        ) : collaborators.data?.viewers.length ? (
          <ul className="space-y-2">
            {collaborators.data.viewers.map((viewer) => (
              <li
                key={viewer.graph_uri}
                className="flex min-h-12 items-center gap-3 rounded-lg border px-3 py-2"
              >
                <span className="flex size-8 shrink-0 items-center justify-center rounded-full bg-muted text-xs font-semibold">
                  {viewer.name.slice(0, 1).toUpperCase()}
                </span>
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-sm font-medium">
                    {viewer.name}
                  </span>
                  <span className="block truncate text-xs text-muted-foreground">
                    @{viewer.username}
                  </span>
                </span>
                {viewer.is_curator && (
                  <Badge variant="secondary">Curator</Badge>
                )}
                {pendingRevoke === viewer.username ? (
                  <div className="flex gap-1">
                    <Button
                      type="button"
                      variant="destructive"
                      size="sm"
                      disabled={revoke.isPending}
                      onClick={() => revoke.mutate(viewer.username)}
                    >
                      Confirm
                    </Button>
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      aria-label="Cancel access revocation"
                      onClick={() => setPendingRevoke(null)}
                    >
                      <X />
                    </Button>
                  </div>
                ) : (
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    aria-label={`Revoke access for ${viewer.name}`}
                    onClick={() => setPendingRevoke(viewer.username)}
                  >
                    <Trash2 />
                  </Button>
                )}
              </li>
            ))}
          </ul>
        ) : (
          <p className="rounded-lg border border-dashed p-3 text-xs leading-5 text-muted-foreground">
            No read-only collaborators have access to this object.
          </p>
        )}
        <MutationError error={grant.error || revoke.error} />

        {canTransfer && (
          <div className="border-t pt-5">
            <Button
              type="button"
              variant="outline"
              size="sm"
              aria-expanded={transferOpen}
              onClick={() => setTransferOpen((open) => !open)}
            >
              <ArrowRightLeft /> Transfer ownership
            </Button>
            {transferOpen && (
              <form
                className="mt-4 space-y-4 rounded-lg border border-warning/25 bg-warning/5 p-4"
                onSubmit={(event) => {
                  event.preventDefault();
                  transfer.mutate();
                }}
              >
                <div>
                  <p className="text-sm font-semibold">
                    Move this private object
                  </p>
                  <p className="mt-1 text-xs leading-5 text-muted-foreground">
                    The recipient becomes the sole owner represented by this
                    command. Your account immediately loses private access.
                  </p>
                </div>
                <div className="space-y-2">
                  <Label htmlFor="transfer-target">
                    New owner username or email
                  </Label>
                  <Input
                    id="transfer-target"
                    value={transferTarget}
                    onChange={(event) => {
                      setTransferTarget(event.target.value);
                      setTransferConfirmation("");
                    }}
                    autoComplete="off"
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="transfer-confirmation">
                    Type the new owner exactly to confirm
                  </Label>
                  <Input
                    id="transfer-confirmation"
                    value={transferConfirmation}
                    onChange={(event) =>
                      setTransferConfirmation(event.target.value)
                    }
                    autoComplete="off"
                  />
                </div>
                <MutationError error={transfer.error} />
                <Button
                  type="submit"
                  variant="destructive"
                  size="sm"
                  disabled={
                    !transferTarget.trim() ||
                    transferConfirmation !== transferTarget ||
                    transfer.isPending
                  }
                >
                  {transfer.isPending ? (
                    <Loader2 className="animate-spin motion-reduce:animate-none" />
                  ) : (
                    <Check />
                  )}
                  {transfer.isPending ? "Transferring…" : "Transfer ownership"}
                </Button>
              </form>
            )}
          </div>
        )}
      </CardContent>
    </Card>
  );
}

function MutationError({ error }: { error: Error | null }) {
  if (!error) return null;
  return (
    <p className="text-xs leading-5 text-destructive" role="alert">
      {error.message}
    </p>
  );
}
