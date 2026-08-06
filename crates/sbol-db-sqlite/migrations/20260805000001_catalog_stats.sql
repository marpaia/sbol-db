-- Exact universal RDF catalog statistics, maintained transactionally.

ALTER TABLE sbol_graphs ADD COLUMN triple_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sbol_graphs ADD COLUMN resource_count INTEGER NOT NULL DEFAULT 0;

UPDATE sbol_graphs
SET triple_count = (SELECT count(*) FROM sbol_triples t WHERE t.graph_iri = sbol_graphs.iri),
    resource_count = (SELECT count(*) FROM accel_object o WHERE o.graph_iri = sbol_graphs.iri);

CREATE INDEX accel_object_global_iri_idx ON accel_object (iri, graph_iri);
CREATE INDEX accel_type_global_type_iri_idx ON accel_type (type_iri, iri, graph_iri);

CREATE TABLE sbol_catalog_stats (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    resources INTEGER NOT NULL CHECK (resources >= 0),
    named_graphs INTEGER NOT NULL CHECK (named_graphs >= 0),
    triples INTEGER NOT NULL CHECK (triples >= 0),
    sequences INTEGER NOT NULL CHECK (sequences >= 0),
    ontologies INTEGER NOT NULL CHECK (ontologies >= 0)
);

INSERT INTO sbol_catalog_stats
SELECT 1,
    (SELECT count(DISTINCT iri) FROM accel_object),
    (SELECT count(*) FROM sbol_graphs),
    (SELECT count(*) FROM sbol_triples),
    (SELECT count(DISTINCT iri) FROM accel_type
      WHERE type_iri IN ('http://sbols.org/v2#Sequence', 'http://sbols.org/v3#Sequence')),
    (SELECT count(*) FROM sbol_ontologies);

CREATE TRIGGER catalog_graph_counter_insert AFTER INSERT ON sbol_graphs BEGIN
    UPDATE sbol_catalog_stats SET named_graphs = named_graphs + 1 WHERE singleton = 1;
END;
CREATE TRIGGER catalog_graph_counter_delete AFTER DELETE ON sbol_graphs BEGIN
    UPDATE sbol_catalog_stats SET named_graphs = named_graphs - 1 WHERE singleton = 1;
END;

CREATE TRIGGER catalog_triple_counter_insert AFTER INSERT ON sbol_triples BEGIN
    UPDATE sbol_catalog_stats SET triples = triples + 1 WHERE singleton = 1;
    UPDATE sbol_graphs SET triple_count = triple_count + 1, updated_at = CURRENT_TIMESTAMP
    WHERE iri = NEW.graph_iri;
END;
CREATE TRIGGER catalog_triple_counter_delete AFTER DELETE ON sbol_triples BEGIN
    UPDATE sbol_catalog_stats SET triples = triples - 1 WHERE singleton = 1;
    UPDATE sbol_graphs SET triple_count = triple_count - 1, updated_at = CURRENT_TIMESTAMP
    WHERE iri = OLD.graph_iri;
END;

CREATE TRIGGER catalog_resource_counter_insert AFTER INSERT ON accel_object BEGIN
    UPDATE sbol_graphs SET resource_count = resource_count + 1 WHERE iri = NEW.graph_iri;
    UPDATE sbol_catalog_stats SET resources = resources + 1
    WHERE singleton = 1 AND (SELECT count(*) FROM accel_object WHERE iri = NEW.iri) = 1;
END;
CREATE TRIGGER catalog_resource_counter_delete AFTER DELETE ON accel_object BEGIN
    UPDATE sbol_graphs SET resource_count = resource_count - 1 WHERE iri = OLD.graph_iri;
    UPDATE sbol_catalog_stats SET resources = resources - 1
    WHERE singleton = 1 AND NOT EXISTS (SELECT 1 FROM accel_object WHERE iri = OLD.iri);
END;

CREATE TRIGGER catalog_sequence_counter_insert AFTER INSERT ON accel_type
WHEN NEW.type_iri IN ('http://sbols.org/v2#Sequence', 'http://sbols.org/v3#Sequence') BEGIN
    UPDATE sbol_catalog_stats SET sequences = sequences + 1
    WHERE singleton = 1 AND (SELECT count(*) FROM accel_type
      WHERE iri = NEW.iri AND type_iri IN
        ('http://sbols.org/v2#Sequence', 'http://sbols.org/v3#Sequence')) = 1;
END;
CREATE TRIGGER catalog_sequence_counter_delete AFTER DELETE ON accel_type
WHEN OLD.type_iri IN ('http://sbols.org/v2#Sequence', 'http://sbols.org/v3#Sequence') BEGIN
    UPDATE sbol_catalog_stats SET sequences = sequences - 1
    WHERE singleton = 1 AND NOT EXISTS (SELECT 1 FROM accel_type
      WHERE iri = OLD.iri AND type_iri IN
        ('http://sbols.org/v2#Sequence', 'http://sbols.org/v3#Sequence'));
END;

CREATE TRIGGER catalog_ontology_counter_insert AFTER INSERT ON sbol_ontologies BEGIN
    UPDATE sbol_catalog_stats SET ontologies = ontologies + 1 WHERE singleton = 1;
END;
CREATE TRIGGER catalog_ontology_counter_delete AFTER DELETE ON sbol_ontologies BEGIN
    UPDATE sbol_catalog_stats SET ontologies = ontologies - 1 WHERE singleton = 1;
END;

CREATE TRIGGER catalog_delete_graph_projection BEFORE DELETE ON sbol_graphs BEGIN
    DELETE FROM accel_dirty WHERE graph_iri = OLD.iri;
    DELETE FROM accel_member WHERE graph_iri = OLD.iri;
    DELETE FROM accel_facet WHERE graph_iri = OLD.iri;
    DELETE FROM accel_role WHERE graph_iri = OLD.iri;
    DELETE FROM accel_type WHERE graph_iri = OLD.iri;
    DELETE FROM accel_object WHERE graph_iri = OLD.iri;
END;
