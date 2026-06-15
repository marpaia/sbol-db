-- A derived-view object and a validation run reference the graph they come
-- from. Name that reference `graph_id`, matching the graph-native model.

ALTER TABLE sbol_objects RENAME COLUMN document_id TO graph_id;
ALTER INDEX sbol_objects_document_class_idx RENAME TO sbol_objects_graph_class_idx;
ALTER TABLE sbol_objects
    RENAME CONSTRAINT sbol_objects_document_id_fkey TO sbol_objects_graph_id_fkey;

ALTER TABLE sbol_validation_runs RENAME COLUMN target_document_id TO graph_id;
ALTER TABLE sbol_validation_runs
    RENAME CONSTRAINT sbol_validation_runs_target_document_id_fkey
    TO sbol_validation_runs_graph_id_fkey;
