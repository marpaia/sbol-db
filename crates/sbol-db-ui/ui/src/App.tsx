import { lazy, Suspense } from "react";
import type { ReactNode } from "react";
import { Navigate, Route, Routes, useParams } from "react-router-dom";

import ProductIdentity from "@/components/portal/ProductIdentity";
import PublicShell from "@/components/portal/PublicShell";
import { Skeleton } from "@/components/ui/skeleton";
import { useInstance } from "@/features/portal/queries";
import { adminPath, publicObjectPath } from "@/lib/routes";
import HomeRoute from "@/routes/HomeRoute";
import LoginRoute from "@/routes/LoginRoute";
import NotFoundRoute from "@/routes/NotFoundRoute";
import PublicAccessGate from "@/routes/PublicAccessGate";
import RegisterRoute from "@/routes/RegisterRoute";
import SetupRoute from "@/routes/SetupRoute";

const PublicObjectRoute = lazy(() => import("@/routes/PublicObjectRoute"));
const AboutRoute = lazy(() => import("@/routes/AboutRoute"));
const ConnectRoute = lazy(() => import("@/routes/ConnectRoute"));
const PrivacyRoute = lazy(() => import("@/routes/PrivacyRoute"));
const TermsRoute = lazy(() => import("@/routes/TermsRoute"));
const ContributionRoute = lazy(() => import("@/routes/ContributionRoute"));
const SearchRoute = lazy(() => import("@/routes/SearchRoute"));
const SequenceSearchRoute = lazy(() => import("@/routes/SequenceSearchRoute"));
const WorkspaceRoute = lazy(() => import("@/routes/WorkspaceRoute"));
const AccountRoute = lazy(() => import("@/routes/AccountRoute"));
const AdminGate = lazy(() => import("@/routes/AdminGate"));
const DashboardRoute = lazy(() => import("@/routes/DashboardRoute"));
const AdminAuditRoute = lazy(() => import("@/routes/AdminAuditRoute"));
const AdminBackupRoute = lazy(() => import("@/routes/AdminBackupRoute"));
const AdminInstanceRoute = lazy(() => import("@/routes/AdminInstanceRoute"));
const AdminIntegrationsRoute = lazy(
  () => import("@/routes/AdminIntegrationsRoute")
);
const AdminSearchRoute = lazy(() => import("@/routes/AdminSearchRoute"));
const AdminUsersRoute = lazy(() => import("@/routes/AdminUsersRoute"));
const GraphDetailRoute = lazy(() => import("@/routes/GraphDetailRoute"));
const GraphsRoute = lazy(() => import("@/routes/GraphsRoute"));
const ImportRoute = lazy(() => import("@/routes/ImportRoute"));
const JobDetailRoute = lazy(() => import("@/routes/JobDetailRoute"));
const JobsRoute = lazy(() => import("@/routes/JobsRoute"));
const LabLayout = lazy(() => import("@/routes/LabLayout"));
const MaintenanceRoute = lazy(() => import("@/routes/MaintenanceRoute"));
const NeighborhoodRoute = lazy(() => import("@/routes/NeighborhoodRoute"));
const ObjectDetailRoute = lazy(() => import("@/routes/ObjectDetailRoute"));
const ObjectLookupRoute = lazy(() => import("@/routes/ObjectLookupRoute"));
const ObjectsRoute = lazy(() => import("@/routes/ObjectsRoute"));
const ObservabilityRoute = lazy(() => import("@/routes/ObservabilityRoute"));
const OntologyDetailRoute = lazy(() => import("@/routes/OntologyDetailRoute"));
const OntologyRoute = lazy(() => import("@/routes/OntologyRoute"));
const SchemaRoute = lazy(() => import("@/routes/SchemaRoute"));
const SequencesRoute = lazy(() => import("@/routes/SequencesRoute"));
const SparqlRoute = lazy(() => import("@/routes/SparqlRoute"));
const SqlRoute = lazy(() => import("@/routes/SqlRoute"));
const TableDetailRoute = lazy(() => import("@/routes/TableDetailRoute"));

export default function App() {
  return (
    <>
      <ProductIdentity />
      <Suspense fallback={<AdminEntryLoading />}>
        <Routes>
          <Route element={<PublicShell />}>
            <Route path="login" element={<LoginRoute />} />
            <Route path="register" element={<RegisterRoute />} />
            <Route path="setup" element={<SetupRoute />} />
            <Route path="connect" element={<ConnectRoute />} />
            <Route path="about" element={<AboutRoute />} />
            <Route path="privacy" element={<PrivacyRoute />} />
            <Route path="terms" element={<TermsRoute />} />
            <Route element={<PublicAccessGate />}>
              <Route index element={<HomeRoute />} />
              <Route
                path="search/*"
                element={
                  <PortalRoute>
                    <SearchRoute />
                  </PortalRoute>
                }
              />
              <Route
                path="sequence-search"
                element={
                  <PortalRoute>
                    <SequenceSearchRoute />
                  </PortalRoute>
                }
              />
              <Route
                path="contribute"
                element={
                  <PortalRoute>
                    <ContributionRoute />
                  </PortalRoute>
                }
              />
              <Route
                path="submit"
                element={<Navigate to="/contribute" replace />}
              />
              <Route
                path="workspace/*"
                element={
                  <PortalRoute>
                    <WorkspaceRoute />
                  </PortalRoute>
                }
              />
              <Route
                path="account"
                element={
                  <PortalRoute>
                    <AccountRoute />
                  </PortalRoute>
                }
              />
              <Route
                path="advanced-search"
                element={<Navigate to="/search" replace />}
              />
              <Route
                path="objects/view/:iri"
                element={
                  <PortalRoute>
                    <PublicObjectRoute />
                  </PortalRoute>
                }
              />
              <Route
                path="public/:collectionId/:displayId/:version?"
                element={<CanonicalObjectRedirect scope="public" />}
              />
              <Route
                path="user/:userId/:collectionId/:displayId/:version?"
                element={<CanonicalObjectRedirect scope="user" />}
              />
            </Route>
            <Route path="*" element={<NotFoundRoute />} />
          </Route>

          <Route path="admin" element={<AdminGate />}>
            <Route element={<LabLayout />}>
              <Route index element={<DashboardRoute />} />
              <Route path="sparql" element={<SparqlRoute />} />
              <Route path="sql" element={<SqlRoute />} />
              <Route path="schema" element={<SchemaRoute />} />
              <Route
                path="schema/tables/:name"
                element={<TableDetailRoute />}
              />
              <Route
                path="schema/tables/:schema/:name"
                element={<RedirectToSchemaTable />}
              />
              <Route path="graphs" element={<GraphsRoute />} />
              <Route path="graphs/:id" element={<GraphDetailRoute />} />
              <Route path="import" element={<ImportRoute />} />
              <Route path="objects" element={<ObjectsRoute />} />
              <Route path="objects/lookup" element={<ObjectLookupRoute />} />
              <Route path="objects/:iri" element={<ObjectDetailRoute />} />
              <Route path="neighborhood" element={<NeighborhoodRoute />} />
              <Route path="sequences" element={<SequencesRoute />} />
              <Route path="ontologies" element={<OntologyRoute />} />
              <Route
                path="ontologies/:prefix"
                element={<OntologyDetailRoute />}
              />
              <Route path="observability" element={<ObservabilityRoute />} />
              <Route path="observability/jobs" element={<JobsRoute />} />
              <Route
                path="observability/jobs/:id"
                element={<JobDetailRoute />}
              />
              <Route
                path="observability/maintenance"
                element={<MaintenanceRoute />}
              />
              <Route
                path="observability/postgres"
                element={
                  <Navigate
                    to={adminPath("/observability/maintenance")}
                    replace
                  />
                }
              />
              <Route
                path="observability/postgres/tables/:schema/:name"
                element={<RedirectToSchemaTable />}
              />
              <Route
                path="settings/instance"
                element={<AdminInstanceRoute />}
              />
              <Route path="settings/users" element={<AdminUsersRoute />} />
              <Route
                path="settings/integrations"
                element={<AdminIntegrationsRoute />}
              />
              <Route path="operations/search" element={<AdminSearchRoute />} />
              <Route path="operations/backup" element={<AdminBackupRoute />} />
              <Route path="operations/audit" element={<AdminAuditRoute />} />
              <Route path="*" element={<Navigate to={adminPath()} replace />} />
            </Route>
          </Route>

          <Route path="lab/*" element={<LegacyLabRedirect />} />
        </Routes>
      </Suspense>
    </>
  );
}

function PortalRoute({ children }: { children: ReactNode }) {
  return <Suspense fallback={<PortalEntryLoading />}>{children}</Suspense>;
}

function PortalEntryLoading() {
  return (
    <div
      className="mx-auto w-full max-w-7xl space-y-5 px-4 py-12 sm:px-6 lg:px-8"
      aria-label="Loading page"
    >
      <Skeleton className="h-5 w-28" />
      <Skeleton className="h-10 w-full max-w-xl" />
      <Skeleton className="h-24 w-full rounded-xl" />
      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
        <Skeleton className="h-48 rounded-xl" />
        <Skeleton className="h-48 rounded-xl" />
        <Skeleton className="h-48 rounded-xl" />
      </div>
    </div>
  );
}

/** Keep bookmarks from the original `/lab` deployment working after cutover. */
function LegacyLabRedirect() {
  const rest = useParams()["*"] || "";
  return <Navigate to={adminPath(rest)} replace />;
}

function RedirectToSchemaTable() {
  const { name } = useParams<{ schema: string; name: string }>();
  return (
    <Navigate
      to={adminPath(`/schema/tables/${encodeURIComponent(name || "")}`)}
      replace
    />
  );
}

function CanonicalObjectRedirect({ scope }: { scope: "public" | "user" }) {
  const instance = useInstance();
  const params = useParams<{
    userId?: string;
    collectionId: string;
    displayId: string;
    version?: string;
  }>();
  if (!instance.data) return null;

  const prefix = instance.data.uri_prefix.replace(/\/+$/, "");
  const segments =
    scope === "public"
      ? ["public", params.collectionId, params.displayId, params.version]
      : [
          "user",
          params.userId,
          params.collectionId,
          params.displayId,
          params.version,
        ];
  const iri = `${prefix}/${segments.filter(Boolean).join("/")}`;
  return <Navigate to={publicObjectPath(iri)} replace />;
}

function AdminEntryLoading() {
  return (
    <div className="flex min-h-svh items-center justify-center bg-background">
      <div className="w-full max-w-sm space-y-3 px-6">
        <Skeleton className="h-10 w-10 rounded-xl" />
        <Skeleton className="h-7 w-48" />
        <Skeleton className="h-4 w-full" />
      </div>
    </div>
  );
}
