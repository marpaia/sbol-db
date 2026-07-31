import { useMemo, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  Check,
  CircleAlert,
  FilePenLine,
  Globe2,
  Loader2,
  Plus,
  Trash2,
  UserRoundCog,
  X,
} from "lucide-react";
import { useNavigate } from "react-router-dom";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/native-select";
import { Textarea } from "@/components/ui/textarea";
import {
  addCollectionMember,
  deleteCollection,
  editPortalObject,
  type PortalObjectDetails,
  publishPortalObject,
  removeCollectionMember,
} from "@/features/portal/api";
import { portalKeys, useSession } from "@/features/portal/queries";
import { shortIri } from "@/features/portal/format";
import { publicObjectPath } from "@/lib/routes";
import { cn } from "@/lib/utils";

type Panel = "edit" | "members" | "publish" | "delete" | null;

export function CollectionManagement({
  object,
}: {
  object: PortalObjectDetails;
}) {
  const session = useSession();
  const user = session.data?.user;
  const collectionLike = object.object_type.endsWith("#Collection");
  const mayManage = Boolean(
    user && (user.is_admin || object.owners.includes(user.graph_uri))
  );

  if (!collectionLike || !mayManage) return null;
  return <ManagementCard object={object} />;
}

function ManagementCard({ object }: { object: PortalObjectDetails }) {
  const [panel, setPanel] = useState<Panel>(null);
  const actions = [
    { id: "edit" as const, label: "Edit details", icon: FilePenLine },
    { id: "members" as const, label: "Members", icon: Plus },
    ...(object.visibility === "restricted"
      ? [{ id: "publish" as const, label: "Publish", icon: Globe2 }]
      : []),
    { id: "delete" as const, label: "Remove", icon: Trash2 },
  ];
  return (
    <Card>
      <CardHeader className="border-b bg-muted/15 p-5">
        <CardTitle className="flex items-center gap-2 text-base">
          <UserRoundCog className="size-4 text-primary" /> Manage collection
        </CardTitle>
        <p className="text-xs leading-5 text-muted-foreground">
          Owner-only collection operations. Every mutation is enforced by the
          server.
        </p>
      </CardHeader>
      <CardContent className="p-2">
        <div className="grid grid-cols-2 gap-1">
          {actions.map((action) => (
            <Button
              key={action.id}
              type="button"
              variant={panel === action.id ? "secondary" : "ghost"}
              size="sm"
              className="justify-start"
              aria-expanded={panel === action.id}
              onClick={() =>
                setPanel((current) =>
                  current === action.id ? null : action.id
                )
              }
            >
              <action.icon /> {action.label}
            </Button>
          ))}
        </div>
        {panel && (
          <div className="mt-2 border-t p-3 pt-4">
            {panel === "edit" && <EditPanel object={object} />}
            {panel === "members" && <MembersPanel object={object} />}
            {panel === "publish" && <PublishPanel object={object} />}
            {panel === "delete" && <DeletePanel object={object} />}
          </div>
        )}
      </CardContent>
    </Card>
  );
}

function EditPanel({ object }: { object: PortalObjectDetails }) {
  const queryClient = useQueryClient();
  const [name, setName] = useState(object.name || "");
  const [description, setDescription] = useState(object.description || "");
  const edit = useMutation({
    mutationFn: () => editPortalObject(object.iri, { name, description }),
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: portalKeys.objectDetails(object.iri),
      });
      await queryClient.invalidateQueries({ queryKey: ["portal", "search"] });
    },
  });
  return (
    <form
      className="space-y-4"
      onSubmit={(event) => {
        event.preventDefault();
        edit.mutate();
      }}
    >
      <PanelHeading
        title="Edit metadata"
        description="Update the collection title and description in place."
      />
      <div className="space-y-2">
        <Label htmlFor="collection-name">Name</Label>
        <Input
          id="collection-name"
          value={name}
          onChange={(event) => setName(event.target.value)}
        />
      </div>
      <div className="space-y-2">
        <Label htmlFor="collection-description">Description</Label>
        <Textarea
          id="collection-description"
          className="min-h-28"
          value={description}
          onChange={(event) => setDescription(event.target.value)}
        />
      </div>
      <MutationError error={edit.error} />
      <Button
        type="submit"
        size="sm"
        className="w-full"
        disabled={edit.isPending}
      >
        {edit.isPending ? (
          <Loader2 className="animate-spin motion-reduce:animate-none" />
        ) : (
          <Check />
        )}
        {edit.isPending ? "Saving…" : edit.isSuccess ? "Saved" : "Save changes"}
      </Button>
    </form>
  );
}

function MembersPanel({ object }: { object: PortalObjectDetails }) {
  const queryClient = useQueryClient();
  const [member, setMember] = useState("");
  const [pendingRemoval, setPendingRemoval] = useState<string | null>(null);
  const refresh = async () => {
    await queryClient.invalidateQueries({
      queryKey: portalKeys.objectDetails(object.iri),
    });
    await queryClient.invalidateQueries({ queryKey: ["portal", "search"] });
  };
  const add = useMutation({
    mutationFn: () => addCollectionMember(object.iri, member.trim()),
    onSuccess: async () => {
      setMember("");
      await refresh();
    },
  });
  const remove = useMutation({
    mutationFn: (iri: string) => removeCollectionMember(object.iri, iri),
    onSuccess: async () => {
      setPendingRemoval(null);
      await refresh();
    },
  });

  return (
    <div className="space-y-4">
      <PanelHeading
        title="Collection members"
        description="Add an existing object IRI or remove one exact membership edge."
      />
      <form
        className="flex gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          if (member.trim()) add.mutate();
        }}
      >
        <Label htmlFor="collection-member" className="sr-only">
          Member IRI
        </Label>
        <Input
          id="collection-member"
          type="url"
          value={member}
          onChange={(event) => setMember(event.target.value)}
          placeholder="https://… object IRI"
        />
        <Button
          type="submit"
          size="icon"
          disabled={!member.trim() || add.isPending}
          aria-label="Add member"
        >
          {add.isPending ? (
            <Loader2 className="animate-spin motion-reduce:animate-none" />
          ) : (
            <Plus />
          )}
        </Button>
      </form>

      {object.members.items.length > 0 ? (
        <ul className="max-h-64 space-y-2 overflow-y-auto pr-1">
          {object.members.items.map((item) => (
            <li key={item.uri} className="rounded-lg border bg-muted/10 p-2.5">
              <div className="flex items-center gap-2">
                <span
                  className="min-w-0 flex-1 truncate text-xs"
                  title={item.uri}
                >
                  {item.name || item.display_id || shortIri(item.uri)}
                </span>
                {pendingRemoval === item.uri ? (
                  <div className="flex gap-1">
                    <Button
                      type="button"
                      variant="destructive"
                      size="sm"
                      disabled={remove.isPending}
                      onClick={() => remove.mutate(item.uri)}
                    >
                      Confirm
                    </Button>
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      aria-label="Cancel removal"
                      onClick={() => setPendingRemoval(null)}
                    >
                      <X />
                    </Button>
                  </div>
                ) : (
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    aria-label={`Remove ${item.name || item.display_id || shortIri(item.uri)} from collection`}
                    onClick={() => setPendingRemoval(item.uri)}
                  >
                    <Trash2 />
                  </Button>
                )}
              </div>
            </li>
          ))}
        </ul>
      ) : (
        <p className="rounded-lg border border-dashed p-3 text-xs leading-5 text-muted-foreground">
          This collection currently has no visible members.
        </p>
      )}
      <MutationError error={add.error || remove.error} />
    </div>
  );
}

function PublishPanel({ object }: { object: PortalObjectDetails }) {
  const navigate = useNavigate();
  const [id, setId] = useState(
    object.display_id?.replace(/_collection$/, "") || ""
  );
  const [version, setVersion] = useState(object.version || "1");
  const [overwrite, setOverwrite] = useState<"fail" | "replace" | "merge">(
    "fail"
  );
  const [acknowledged, setAcknowledged] = useState(false);
  const publish = useMutation({
    mutationFn: () =>
      publishPortalObject(object.iri, {
        id: id.trim(),
        version: version.trim(),
        name: object.name || undefined,
        description: object.description || undefined,
        citations: object.provenance.citations,
        overwrite,
      }),
    onSuccess: (result) => navigate(publicObjectPath(result.collection_uri)),
  });
  return (
    <form
      className="space-y-4"
      onSubmit={(event) => {
        event.preventDefault();
        publish.mutate();
      }}
    >
      <PanelHeading
        title="Publish collection"
        description="Mint public identities, write the public copy, then remove this private submission."
      />
      <div className="space-y-2">
        <Label htmlFor="publish-id">Public ID</Label>
        <Input
          id="publish-id"
          value={id}
          onChange={(event) => setId(event.target.value)}
        />
      </div>
      <div className="space-y-2">
        <Label htmlFor="publish-version">Version</Label>
        <Input
          id="publish-version"
          value={version}
          onChange={(event) => setVersion(event.target.value)}
        />
      </div>
      <div className="space-y-2">
        <Label htmlFor="publish-overwrite">If public identity exists</Label>
        <NativeSelect
          id="publish-overwrite"
          value={overwrite}
          onChange={(event) =>
            setOverwrite(event.target.value as typeof overwrite)
          }
        >
          <option value="fail">Stop and report conflict</option>
          <option value="replace">Add published replacement</option>
          <option value="merge">Merge into public graph</option>
        </NativeSelect>
      </div>
      <label className="flex cursor-pointer items-start gap-2.5 rounded-lg border border-amber-500/25 bg-amber-500/5 p-3 text-xs leading-5">
        <input
          type="checkbox"
          className="mt-0.5 size-4 accent-[hsl(var(--primary))]"
          checked={acknowledged}
          onChange={(event) => setAcknowledged(event.target.checked)}
        />
        <span>
          I understand the private identity will be replaced by newly minted
          public identities.
        </span>
      </label>
      <MutationError error={publish.error} />
      <Button
        type="submit"
        size="sm"
        className="w-full"
        disabled={
          !id.trim() || !version.trim() || !acknowledged || publish.isPending
        }
      >
        {publish.isPending ? (
          <Loader2 className="animate-spin motion-reduce:animate-none" />
        ) : (
          <Globe2 />
        )}
        {publish.isPending ? "Publishing…" : "Publish collection"}
      </Button>
    </form>
  );
}

function DeletePanel({ object }: { object: PortalObjectDetails }) {
  const navigate = useNavigate();
  const [confirmation, setConfirmation] = useState("");
  const target = useMemo(
    () => object.display_id || shortIri(object.iri),
    [object.display_id, object.iri]
  );
  const remove = useMutation({
    mutationFn: () => deleteCollection(object.iri),
    onSuccess: () => navigate("/workspace", { replace: true }),
  });
  return (
    <form
      className="space-y-4"
      onSubmit={(event) => {
        event.preventDefault();
        remove.mutate();
      }}
    >
      <PanelHeading
        title="Remove collection"
        description="Deletes this collection and its owned closure. External references are not rewritten."
        destructive
      />
      <div className="rounded-lg border border-destructive/25 bg-destructive/5 p-3">
        <div className="text-[11px] font-medium text-destructive">
          Exact target
        </div>
        <code className="mt-1 block break-all font-mono text-[10px] leading-4">
          {object.iri}
        </code>
      </div>
      <div className="space-y-2">
        <Label htmlFor="delete-confirmation">
          Type <span className="font-mono">{target}</span> to confirm
        </Label>
        <Input
          id="delete-confirmation"
          autoComplete="off"
          value={confirmation}
          onChange={(event) => setConfirmation(event.target.value)}
        />
      </div>
      <MutationError error={remove.error} />
      <Button
        type="submit"
        variant="destructive"
        size="sm"
        className="w-full"
        disabled={confirmation !== target || remove.isPending}
      >
        {remove.isPending ? (
          <Loader2 className="animate-spin motion-reduce:animate-none" />
        ) : (
          <Trash2 />
        )}
        {remove.isPending ? "Removing…" : "Remove this collection"}
      </Button>
    </form>
  );
}

function PanelHeading({
  title,
  description,
  destructive = false,
}: {
  title: string;
  description: string;
  destructive?: boolean;
}) {
  return (
    <div>
      <h3
        className={cn(
          "text-sm font-semibold",
          destructive && "text-destructive"
        )}
      >
        {title}
      </h3>
      <p className="mt-1 text-xs leading-5 text-muted-foreground">
        {description}
      </p>
    </div>
  );
}

function MutationError({ error }: { error: Error | null }) {
  if (!error) return null;
  return (
    <div
      role="alert"
      className="flex gap-2 rounded-lg border border-destructive/25 bg-destructive/5 p-3 text-xs leading-5 text-destructive"
    >
      <CircleAlert className="mt-0.5 size-3.5 shrink-0" />
      <span>{error.message}</span>
    </div>
  );
}
