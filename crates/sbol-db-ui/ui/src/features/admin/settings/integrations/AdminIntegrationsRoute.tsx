import { Cable, RefreshCw, Trash2 } from "lucide-react";
import { useMemo, useState } from "react";

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
import { NativeSelect } from "@/components/ui/native-select";
import { Skeleton } from "@/components/ui/skeleton";
import { Textarea } from "@/components/ui/textarea";
import {
  useAdminIntegrations,
  useDeletePlugin,
  useDeleteRegistry,
  useDeleteRemote,
  useJoinFederation,
  useSavePlugin,
  useSaveRegistry,
  useSaveRemote,
  useSyncFederation,
} from "@/features/admin/settings/integrations/queries";

const PLUGIN_CATEGORIES = [
  "rendering",
  "download",
  "submit",
  "curation",
  "authorization",
];

export default function AdminIntegrationsRoute() {
  const query = useAdminIntegrations();

  return (
    <AdminPage
      title="Integrations"
      description="Connect registry discovery, external data sources, and extension services. Stored credentials are accepted on writes but redacted from every read response."
    >
      {query.error ? (
        <SurfaceState
          variant="error"
          title="Integration settings unavailable"
          description={(query.error as Error).message}
        />
      ) : query.isLoading || !query.data ? (
        <div className="space-y-6">
          <Skeleton className="h-52 rounded-xl" />
          <Skeleton className="h-64 rounded-xl" />
          <Skeleton className="h-64 rounded-xl" />
        </div>
      ) : (
        <>
          <FederationSection data={query.data.federation} />
          <RegistriesSection items={query.data.registries} />
          <RemotesSection items={query.data.remotes} />
          <PluginsSection items={query.data.plugins} />
        </>
      )}
    </AdminPage>
  );
}

function FederationSection({
  data,
}: {
  data: { registered: boolean; url: string };
}) {
  const join = useJoinFederation();
  const sync = useSyncFederation();
  const [email, setEmail] = useState("");
  const [url, setUrl] = useState(data.url);

  return (
    <AdminSection
      title="Web of Registries"
      description="Advertise this instance and synchronize URI-prefix routing with a registry directory."
      action={
        <Badge variant={data.registered ? "default" : "secondary"}>
          {data.registered ? "Joined" : "Not joined"}
        </Badge>
      }
    >
      <form
        className="grid gap-4 md:grid-cols-[1fr_1fr_auto]"
        onSubmit={(event) => {
          event.preventDefault();
          join.mutate({ administratorEmail: email, url });
        }}
      >
        <Field label="Administrator email">
          <Input
            type="email"
            value={email}
            onChange={(event) => setEmail(event.target.value)}
            required
          />
        </Field>
        <Field label="Directory URL">
          <Input
            value={url}
            onChange={(event) => setUrl(event.target.value)}
            required
            inputMode="url"
          />
        </Field>
        <div className="flex items-end gap-2">
          <Button type="submit" disabled={join.isPending}>
            <Cable /> {data.registered ? "Rejoin" : "Join"}
          </Button>
          <Button
            type="button"
            variant="outline"
            disabled={!data.registered || sync.isPending}
            onClick={() => sync.mutate()}
          >
            <RefreshCw className={sync.isPending ? "animate-spin" : ""} /> Sync
          </Button>
        </div>
        <div className="md:col-span-3">
          <MutationStatus
            pending={join.isPending || sync.isPending}
            error={join.error || sync.error}
            success={
              join.isSuccess
                ? "Federation membership saved."
                : sync.data
                  ? `${sync.data.count} registry entries synchronized.`
                  : null
            }
          />
        </div>
      </form>
    </AdminSection>
  );
}

function RegistriesSection({
  items,
}: {
  items: Array<{ uri: string; url: string }>;
}) {
  const save = useSaveRegistry();
  const remove = useDeleteRegistry();
  const [uri, setUri] = useState("");
  const [url, setUrl] = useState("");

  return (
    <AdminSection
      title="Registry routes"
      description="Map object URI prefixes to the instances that serve them. More-specific prefixes win during resolution."
    >
      <form
        className="grid gap-3 md:grid-cols-[1fr_1fr_auto]"
        onSubmit={(event) => {
          event.preventDefault();
          save.mutate(
            { uri, url },
            {
              onSuccess: () => {
                setUri("");
                setUrl("");
              },
            }
          );
        }}
      >
        <Input
          value={uri}
          onChange={(event) => setUri(event.target.value)}
          placeholder="https://identifiers.example/"
          required
        />
        <Input
          value={url}
          onChange={(event) => setUrl(event.target.value)}
          placeholder="https://registry.example.org"
          required
        />
        <Button type="submit" disabled={save.isPending}>
          Save route
        </Button>
      </form>
      <div className="mt-4">
        <MutationStatus pending={save.isPending} error={save.error} />
      </div>
      <div className="mt-5 divide-y rounded-lg border">
        {items.length === 0 ? (
          <p className="px-4 py-6 text-center text-xs text-muted-foreground">
            No registry routes configured.
          </p>
        ) : (
          items.map((item) => (
            <div
              key={item.uri}
              className="flex flex-wrap items-center gap-3 px-4 py-3"
            >
              <div className="min-w-0 flex-1">
                <code className="block truncate text-xs font-medium">
                  {item.uri}
                </code>
                <span className="mt-1 block truncate text-xs text-muted-foreground">
                  {item.url}
                </span>
              </div>
              <DeleteControl
                expected={`DELETE REGISTRY ${item.uri}`}
                pending={remove.isPending && remove.variables?.uri === item.uri}
                error={remove.variables?.uri === item.uri ? remove.error : null}
                onDelete={(confirmation) =>
                  remove.mutate({ uri: item.uri, confirmation })
                }
              />
            </div>
          ))
        )}
      </div>
    </AdminSection>
  );
}

function RemotesSection({
  items,
}: {
  items: Record<string, Record<string, unknown>>;
}) {
  const save = useSaveRemote();
  const remove = useDeleteRemote();
  const [raw, setRaw] = useState(
    '{\n  "id": "",\n  "type": "ice",\n  "url": ""\n}'
  );
  const [parseError, setParseError] = useState<Error | null>(null);
  const entries = Object.entries(items);

  return (
    <AdminSection
      title="External remotes"
      description="Configure ICE or Benchling sources. Secret fields render as [redacted] and must be supplied again when replacing a configuration."
    >
      <form
        className="grid gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          try {
            const parsed = JSON.parse(raw) as Record<string, unknown>;
            setParseError(null);
            save.mutate(parsed);
          } catch (error) {
            setParseError(
              error instanceof Error ? error : new Error("Invalid JSON")
            );
          }
        }}
      >
        <Textarea
          value={raw}
          onChange={(event) => setRaw(event.target.value)}
          rows={7}
          className="font-mono text-xs"
          spellCheck={false}
        />
        <div className="flex flex-wrap items-center justify-between gap-3">
          <MutationStatus
            pending={save.isPending}
            error={parseError || save.error}
            success={save.isSuccess ? "Remote saved." : null}
          />
          <Button type="submit" disabled={save.isPending}>
            Save remote JSON
          </Button>
        </div>
      </form>
      <div className="mt-5 grid gap-3 md:grid-cols-2">
        {entries.length === 0 ? (
          <p className="md:col-span-2 rounded-lg border border-dashed px-4 py-6 text-center text-xs text-muted-foreground">
            No remotes configured.
          </p>
        ) : (
          entries.map(([id, remote]) => (
            <article key={id} className="rounded-lg border bg-background p-4">
              <div className="flex items-center gap-2">
                <code className="font-semibold">{id}</code>
                <Badge variant="secondary">
                  {String(remote.type ?? "remote")}
                </Badge>
              </div>
              <pre className="mt-3 max-h-40 overflow-auto rounded-md bg-muted/40 p-3 text-[11px] leading-5">
                {JSON.stringify(remote, null, 2)}
              </pre>
              <div className="mt-3">
                <DeleteControl
                  expected={`DELETE REMOTE ${id}`}
                  pending={remove.isPending && remove.variables?.id === id}
                  error={remove.variables?.id === id ? remove.error : null}
                  onDelete={(confirmation) =>
                    remove.mutate({ id, confirmation })
                  }
                />
              </div>
            </article>
          ))
        )}
      </div>
    </AdminSection>
  );
}

function PluginsSection({
  items,
}: {
  items: Record<string, Array<{ name: string; url: string }>>;
}) {
  const save = useSavePlugin();
  const remove = useDeletePlugin();
  const [category, setCategory] = useState("rendering");
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const rows = useMemo(
    () =>
      Object.entries(items).flatMap(([group, plugins]) =>
        plugins.map((plugin, index) => ({
          ...plugin,
          category: group,
          id: String(index + 1),
        }))
      ),
    [items]
  );

  return (
    <AdminSection
      title="Plugins"
      description="Register extension services by category. Outbound calls remain subject to the server's HTTPS and SSRF policy."
    >
      <form
        className="grid gap-3 md:grid-cols-[180px_1fr_1fr_auto]"
        onSubmit={(event) => {
          event.preventDefault();
          save.mutate(
            { category, id: "New", name, url },
            {
              onSuccess: () => {
                setName("");
                setUrl("");
              },
            }
          );
        }}
      >
        <NativeSelect
          value={category}
          onChange={(event) => setCategory(event.target.value)}
        >
          {PLUGIN_CATEGORIES.map((value) => (
            <option key={value} value={value}>
              {value}
            </option>
          ))}
        </NativeSelect>
        <Input
          value={name}
          onChange={(event) => setName(event.target.value)}
          placeholder="Plugin name"
          required
        />
        <Input
          value={url}
          onChange={(event) => setUrl(event.target.value)}
          placeholder="https://plugin.example/"
          required
        />
        <Button type="submit" disabled={save.isPending}>
          Add plugin
        </Button>
      </form>
      <div className="mt-4">
        <MutationStatus pending={save.isPending} error={save.error} />
      </div>
      <div className="mt-5 divide-y rounded-lg border">
        {rows.length === 0 ? (
          <p className="px-4 py-6 text-center text-xs text-muted-foreground">
            No plugins configured.
          </p>
        ) : (
          rows.map((plugin) => {
            const target = `${plugin.category}/${plugin.id}`;
            return (
              <div
                key={target}
                className="flex flex-wrap items-center gap-3 px-4 py-3"
              >
                <Badge variant="secondary">{plugin.category}</Badge>
                <div className="min-w-0 flex-1">
                  <p className="text-sm font-medium">{plugin.name}</p>
                  <p className="truncate text-xs text-muted-foreground">
                    {plugin.url}
                  </p>
                </div>
                <DeleteControl
                  expected={`DELETE PLUGIN ${target}`}
                  pending={
                    remove.isPending &&
                    `${remove.variables?.category}/${remove.variables?.id}` ===
                      target
                  }
                  error={
                    `${remove.variables?.category}/${remove.variables?.id}` ===
                    target
                      ? remove.error
                      : null
                  }
                  onDelete={(confirmation) =>
                    remove.mutate({
                      category: plugin.category,
                      id: plugin.id,
                      confirmation,
                    })
                  }
                />
              </div>
            );
          })
        )}
      </div>
    </AdminSection>
  );
}

function DeleteControl({
  expected,
  pending,
  error,
  onDelete,
}: {
  expected: string;
  pending: boolean;
  error: unknown;
  onDelete: (confirmation: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [value, setValue] = useState("");
  if (!open) {
    return (
      <Button
        type="button"
        size="sm"
        variant="ghost"
        className="text-destructive hover:bg-destructive/10 hover:text-destructive"
        onClick={() => setOpen(true)}
      >
        <Trash2 /> Remove
      </Button>
    );
  }
  return (
    <div className="w-full rounded-md border border-destructive/25 bg-destructive/5 p-3 sm:w-auto sm:min-w-80">
      <p className="text-[11px] text-muted-foreground">
        Type <code className="font-semibold text-foreground">{expected}</code>
      </p>
      <div className="mt-2 flex gap-2">
        <Input
          value={value}
          onChange={(event) => setValue(event.target.value)}
          className="h-8 font-mono text-xs"
        />
        <Button
          type="button"
          size="sm"
          variant="destructive"
          disabled={pending || value !== expected}
          onClick={() => onDelete(value)}
        >
          Delete
        </Button>
        <Button
          type="button"
          size="sm"
          variant="ghost"
          onClick={() => setOpen(false)}
        >
          Cancel
        </Button>
      </div>
      {error != null && (
        <div className="mt-2">
          <MutationStatus error={error} />
        </div>
      )}
    </div>
  );
}
