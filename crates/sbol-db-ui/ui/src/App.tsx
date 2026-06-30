import { Navigate, Route, Routes, useParams } from "react-router-dom";

import DashboardRoute from "@/routes/DashboardRoute";
import GraphDetailRoute from "@/routes/GraphDetailRoute";
import GraphsRoute from "@/routes/GraphsRoute";
import ImportRoute from "@/routes/ImportRoute";
import JobDetailRoute from "@/routes/JobDetailRoute";
import JobsRoute from "@/routes/JobsRoute";
import LabLayout from "@/routes/LabLayout";
import MaintenanceRoute from "@/routes/MaintenanceRoute";
import NeighborhoodRoute from "@/routes/NeighborhoodRoute";
import ObjectDetailRoute from "@/routes/ObjectDetailRoute";
import ObjectLookupRoute from "@/routes/ObjectLookupRoute";
import ObjectsRoute from "@/routes/ObjectsRoute";
import ObservabilityRoute from "@/routes/ObservabilityRoute";
import OntologyDetailRoute from "@/routes/OntologyDetailRoute";
import OntologyRoute from "@/routes/OntologyRoute";
import SchemaRoute from "@/routes/SchemaRoute";
import TableDetailRoute from "@/routes/TableDetailRoute";
import SequencesRoute from "@/routes/SequencesRoute";
import SparqlRoute from "@/routes/SparqlRoute";
import SqlRoute from "@/routes/SqlRoute";

export default function App() {
  return (
    <Routes>
      <Route path="/" element={<LabLayout />}>
        <Route index element={<DashboardRoute />} />
        <Route path="sparql" element={<SparqlRoute />} />
        <Route path="sql" element={<SqlRoute />} />
        <Route path="schema" element={<SchemaRoute />} />
        <Route path="schema/tables/:name" element={<TableDetailRoute />} />
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
        <Route path="ontologies/:prefix" element={<OntologyDetailRoute />} />
        <Route path="observability" element={<ObservabilityRoute />} />
        <Route path="observability/jobs" element={<JobsRoute />} />
        <Route path="observability/jobs/:id" element={<JobDetailRoute />} />
        <Route
          path="observability/maintenance"
          element={<MaintenanceRoute />}
        />
        <Route
          path="observability/postgres"
          element={<Navigate to="/observability/maintenance" replace />}
        />
        <Route
          path="observability/postgres/tables/:schema/:name"
          element={<RedirectToSchemaTable />}
        />
        <Route path="*" element={<Navigate to="/" replace />} />
      </Route>
    </Routes>
  );
}

/**
 * The canonical table detail URL is `/schema/tables/:name`; SQL schemas
 * aren't a domain concept in the UI. Redirect the schema-qualified
 * shapes (under `/observability/postgres/tables` and `/schema/tables`)
 * to the canonical form so bookmarks survive.
 */
function RedirectToSchemaTable() {
  const { name } = useParams<{ schema: string; name: string }>();
  return (
    <Navigate to={`/schema/tables/${encodeURIComponent(name ?? "")}`} replace />
  );
}
