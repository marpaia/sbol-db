import { Navigate, Route, Routes } from "react-router-dom";

import DashboardRoute from "@/routes/DashboardRoute";
import LabLayout from "@/routes/LabLayout";
import OntologyRoute from "@/routes/OntologyRoute";
import SchemaRoute from "@/routes/SchemaRoute";
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
        <Route path="ontologies" element={<OntologyRoute />} />
        <Route path="*" element={<Navigate to="/" replace />} />
      </Route>
    </Routes>
  );
}
