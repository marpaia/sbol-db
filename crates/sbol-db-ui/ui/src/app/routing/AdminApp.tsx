import { lazy, Suspense, type ReactNode } from "react";
import { Navigate, Route, Routes, useParams } from "react-router-dom";

import { Skeleton } from "@/components/ui/skeleton";
import {
  adminRouteSegment,
  isAdminDestinationAvailable,
  type AdminDestinationId,
} from "@/app/routing/adminManifest";
import { useBackendInfo } from "@/features/admin/backend/queries";
import { adminPath } from "@/lib/routes";

const AdminGate = lazy(() => import("@/routes/AdminGate"));
const DashboardRoute = lazy(
  () => import("@/features/admin/overview/DashboardRoute")
);
const AdminAuditRoute = lazy(
  () => import("@/features/admin/settings/audit/AdminAuditRoute")
);
const AdminBackupRoute = lazy(
  () => import("@/features/admin/settings/backup/AdminBackupRoute")
);
const AdminEdgeRoute = lazy(
  () => import("@/features/admin/settings/edge/AdminEdgeRoute")
);
const AdminInstanceRoute = lazy(
  () => import("@/features/admin/settings/instance/AdminInstanceRoute")
);
const AdminIntegrationsRoute = lazy(
  () => import("@/features/admin/settings/integrations/AdminIntegrationsRoute")
);
const AdminSearchRoute = lazy(
  () => import("@/features/admin/settings/search/AdminSearchRoute")
);
const AdminUsersRoute = lazy(
  () => import("@/features/admin/settings/users/AdminUsersRoute")
);
const GraphDetailRoute = lazy(
  () => import("@/features/admin/graphs/GraphDetailRoute")
);
const GraphsRoute = lazy(() => import("@/features/admin/graphs/GraphsRoute"));
const ImportRoute = lazy(() => import("@/features/admin/imports/ImportRoute"));
const JobDetailRoute = lazy(
  () => import("@/features/admin/jobs/JobDetailRoute")
);
const JobsRoute = lazy(() => import("@/features/admin/jobs/JobsRoute"));
const LabLayout = lazy(() => import("@/routes/LabLayout"));
const MaintenanceRoute = lazy(
  () => import("@/features/admin/maintenance/MaintenanceRoute")
);
const NeighborhoodRoute = lazy(
  () => import("@/features/admin/neighborhood/NeighborhoodRoute")
);
const ObjectDetailRoute = lazy(
  () => import("@/features/admin/objects/ObjectDetailRoute")
);
const ObjectLookupRoute = lazy(
  () => import("@/features/admin/objects/ObjectLookupRoute")
);
const ObjectsRoute = lazy(
  () => import("@/features/admin/objects/ObjectsRoute")
);
const ObservabilityRoute = lazy(
  () => import("@/features/admin/observability/ObservabilityRoute")
);
const OntologyDetailRoute = lazy(
  () => import("@/features/admin/ontologies/OntologyDetailRoute")
);
const OntologyRoute = lazy(
  () => import("@/features/admin/ontologies/OntologyRoute")
);
const SchemaRoute = lazy(() => import("@/features/admin/schema/SchemaRoute"));
const SequencesRoute = lazy(
  () => import("@/features/admin/sequences/SequencesRoute")
);
const SparqlRoute = lazy(
  () => import("@/features/admin/workbench/SparqlRoute")
);
const SqlRoute = lazy(() => import("@/features/admin/workbench/SqlRoute"));
const TableDetailRoute = lazy(
  () => import("@/features/admin/schema/TableDetailRoute")
);

export default function AdminApp() {
  return (
    <Suspense fallback={<AdminEntryLoading />}>
      <Routes>
        <Route element={<AdminGate />}>
          <Route element={<LabLayout />}>
            <Route index element={<DashboardRoute />} />
            <Route
              path={adminRouteSegment("sparql")}
              element={<SparqlRoute />}
            />
            <Route
              path={adminRouteSegment("sql")}
              element={
                <AvailableRoute id="sql">
                  <SqlRoute />
                </AvailableRoute>
              }
            />
            <Route
              path={adminRouteSegment("schema")}
              element={
                <AvailableRoute id="schema">
                  <SchemaRoute />
                </AvailableRoute>
              }
            />
            <Route
              path={`${adminRouteSegment("schema")}/tables/:name`}
              element={
                <AvailableRoute id="schema">
                  <TableDetailRoute />
                </AvailableRoute>
              }
            />
            <Route
              path={`${adminRouteSegment("schema")}/tables/:schema/:name`}
              element={<RedirectToSchemaTable />}
            />
            <Route
              path={adminRouteSegment("graphs")}
              element={<GraphsRoute />}
            />
            <Route
              path={`${adminRouteSegment("graphs")}/:id`}
              element={<GraphDetailRoute />}
            />
            <Route
              path={adminRouteSegment("import")}
              element={<ImportRoute />}
            />
            <Route
              path={adminRouteSegment("objects")}
              element={<ObjectsRoute />}
            />
            <Route
              path={adminRouteSegment("object-lookup")}
              element={<ObjectLookupRoute />}
            />
            <Route
              path={`${adminRouteSegment("objects")}/:iri`}
              element={<ObjectDetailRoute />}
            />
            <Route
              path={adminRouteSegment("neighborhood")}
              element={<NeighborhoodRoute />}
            />
            <Route
              path={adminRouteSegment("sequences")}
              element={<SequencesRoute />}
            />
            <Route
              path={adminRouteSegment("ontologies")}
              element={<OntologyRoute />}
            />
            <Route
              path={`${adminRouteSegment("ontologies")}/:prefix`}
              element={<OntologyDetailRoute />}
            />
            <Route
              path={adminRouteSegment("metrics")}
              element={<ObservabilityRoute />}
            />
            <Route path={adminRouteSegment("jobs")} element={<JobsRoute />} />
            <Route
              path={`${adminRouteSegment("jobs")}/:id`}
              element={<JobDetailRoute />}
            />
            <Route
              path={adminRouteSegment("maintenance")}
              element={
                <AvailableRoute id="maintenance">
                  <MaintenanceRoute />
                </AvailableRoute>
              }
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
              path={adminRouteSegment("instance")}
              element={<AdminInstanceRoute />}
            />
            <Route
              path={adminRouteSegment("users")}
              element={<AdminUsersRoute />}
            />
            <Route
              path={adminRouteSegment("edge")}
              element={<AdminEdgeRoute />}
            />
            <Route
              path={adminRouteSegment("integrations")}
              element={<AdminIntegrationsRoute />}
            />
            <Route
              path={adminRouteSegment("search-indexes")}
              element={<AdminSearchRoute />}
            />
            <Route
              path={adminRouteSegment("backups")}
              element={<AdminBackupRoute />}
            />
            <Route
              path={adminRouteSegment("audit")}
              element={<AdminAuditRoute />}
            />
            <Route path="*" element={<Navigate to={adminPath()} replace />} />
          </Route>
        </Route>
      </Routes>
    </Suspense>
  );
}

function AvailableRoute({
  id,
  children,
}: {
  id: AdminDestinationId;
  children: ReactNode;
}) {
  const info = useBackendInfo();
  if (info.isLoading) return <AdminEntryLoading />;
  if (!isAdminDestinationAvailable(id, info.data)) {
    return <Navigate to={adminPath()} replace />;
  }
  return children;
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
