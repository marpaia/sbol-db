-- Deleting accelerator rows from the original BEFORE DELETE graph trigger
-- fires their counter triggers while the graph tuple itself is already being
-- deleted. PostgreSQL rejects that attempt to update the same tuple. Run the
-- projection cleanup after the graph row is gone instead: per-graph counter
-- updates then match no row, while global resource and sequence counters still
-- observe every accelerator-row deletion.

DROP TRIGGER catalog_delete_graph_projection ON sbol_graphs;

CREATE TRIGGER catalog_delete_graph_projection
AFTER DELETE ON sbol_graphs FOR EACH ROW EXECUTE FUNCTION catalog_delete_graph_projection();
