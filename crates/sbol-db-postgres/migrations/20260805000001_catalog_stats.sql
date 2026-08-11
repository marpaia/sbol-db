-- Exact universal RDF catalog statistics. The singleton row and graph-local
-- counts are maintained in the same transaction as canonical triples and the
-- RDF-derived accelerator/catalog rows, making dashboard reads constant-time.

ALTER TABLE sbol_graphs
    ADD COLUMN triple_count bigint NOT NULL DEFAULT 0,
    ADD COLUMN resource_count bigint NOT NULL DEFAULT 0;

UPDATE sbol_graphs g
SET triple_count = (SELECT count(*) FROM sbol_triples t WHERE t.graph_iri = g.iri),
    resource_count = (SELECT count(*) FROM accel_object o WHERE o.graph_iri = g.iri);

CREATE INDEX accel_object_global_iri_idx ON accel_object (iri, graph_iri);
CREATE INDEX accel_type_global_type_iri_idx ON accel_type (type_iri, iri, graph_iri);

CREATE TABLE sbol_catalog_stats (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    resources bigint NOT NULL CHECK (resources >= 0),
    named_graphs bigint NOT NULL CHECK (named_graphs >= 0),
    triples bigint NOT NULL CHECK (triples >= 0),
    sequences bigint NOT NULL CHECK (sequences >= 0),
    ontologies bigint NOT NULL CHECK (ontologies >= 0)
);

INSERT INTO sbol_catalog_stats (
    resources, named_graphs, triples, sequences, ontologies
)
SELECT
    (SELECT count(DISTINCT iri) FROM accel_object),
    (SELECT count(*) FROM sbol_graphs),
    (SELECT count(*) FROM sbol_triples),
    (SELECT count(DISTINCT iri) FROM accel_type
      WHERE type_iri IN ('http://sbols.org/v2#Sequence', 'http://sbols.org/v3#Sequence')),
    (SELECT count(*) FROM sbol_ontologies);

CREATE FUNCTION catalog_graph_counter() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        UPDATE sbol_catalog_stats SET named_graphs = named_graphs + 1 WHERE singleton;
        RETURN NEW;
    END IF;
    UPDATE sbol_catalog_stats SET named_graphs = named_graphs - 1 WHERE singleton;
    RETURN OLD;
END;
$$;
CREATE TRIGGER catalog_graph_counter_insert
AFTER INSERT ON sbol_graphs FOR EACH ROW EXECUTE FUNCTION catalog_graph_counter();
CREATE TRIGGER catalog_graph_counter_delete
AFTER DELETE ON sbol_graphs FOR EACH ROW EXECUTE FUNCTION catalog_graph_counter();

CREATE FUNCTION catalog_triple_counter() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        UPDATE sbol_catalog_stats SET triples = triples + 1 WHERE singleton;
        IF NEW.graph_iri IS NOT NULL THEN
            UPDATE sbol_graphs SET triple_count = triple_count + 1, updated_at = now()
            WHERE iri = NEW.graph_iri;
        END IF;
        RETURN NEW;
    END IF;
    UPDATE sbol_catalog_stats SET triples = triples - 1 WHERE singleton;
    IF OLD.graph_iri IS NOT NULL THEN
        UPDATE sbol_graphs SET triple_count = triple_count - 1, updated_at = now()
        WHERE iri = OLD.graph_iri;
    END IF;
    RETURN OLD;
END;
$$;
CREATE TRIGGER catalog_triple_counter_insert
AFTER INSERT ON sbol_triples FOR EACH ROW EXECUTE FUNCTION catalog_triple_counter();
CREATE TRIGGER catalog_triple_counter_delete
AFTER DELETE ON sbol_triples FOR EACH ROW EXECUTE FUNCTION catalog_triple_counter();

CREATE FUNCTION catalog_resource_counter() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        UPDATE sbol_graphs SET resource_count = resource_count + 1 WHERE iri = NEW.graph_iri;
        IF (SELECT count(*) FROM accel_object WHERE iri = NEW.iri) = 1 THEN
            UPDATE sbol_catalog_stats SET resources = resources + 1 WHERE singleton;
        END IF;
        RETURN NEW;
    END IF;
    UPDATE sbol_graphs SET resource_count = resource_count - 1 WHERE iri = OLD.graph_iri;
    IF NOT EXISTS (SELECT 1 FROM accel_object WHERE iri = OLD.iri) THEN
        UPDATE sbol_catalog_stats SET resources = resources - 1 WHERE singleton;
    END IF;
    RETURN OLD;
END;
$$;
CREATE TRIGGER catalog_resource_counter_insert
AFTER INSERT ON accel_object FOR EACH ROW EXECUTE FUNCTION catalog_resource_counter();
CREATE TRIGGER catalog_resource_counter_delete
AFTER DELETE ON accel_object FOR EACH ROW EXECUTE FUNCTION catalog_resource_counter();

CREATE FUNCTION catalog_sequence_counter() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    sequence_type boolean;
BEGIN
    IF TG_OP = 'INSERT' THEN
        sequence_type := NEW.type_iri IN
            ('http://sbols.org/v2#Sequence', 'http://sbols.org/v3#Sequence');
    ELSE
        sequence_type := OLD.type_iri IN
            ('http://sbols.org/v2#Sequence', 'http://sbols.org/v3#Sequence');
    END IF;
    IF NOT sequence_type THEN
        IF TG_OP = 'INSERT' THEN RETURN NEW; ELSE RETURN OLD; END IF;
    END IF;
    IF TG_OP = 'INSERT' THEN
        IF (SELECT count(*) FROM accel_type
            WHERE iri = NEW.iri AND type_iri IN
              ('http://sbols.org/v2#Sequence', 'http://sbols.org/v3#Sequence')) = 1 THEN
            UPDATE sbol_catalog_stats SET sequences = sequences + 1 WHERE singleton;
        END IF;
        RETURN NEW;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM accel_type
        WHERE iri = OLD.iri AND type_iri IN
          ('http://sbols.org/v2#Sequence', 'http://sbols.org/v3#Sequence')) THEN
        UPDATE sbol_catalog_stats SET sequences = sequences - 1 WHERE singleton;
    END IF;
    RETURN OLD;
END;
$$;
CREATE TRIGGER catalog_sequence_counter_insert
AFTER INSERT ON accel_type FOR EACH ROW EXECUTE FUNCTION catalog_sequence_counter();
CREATE TRIGGER catalog_sequence_counter_delete
AFTER DELETE ON accel_type FOR EACH ROW EXECUTE FUNCTION catalog_sequence_counter();

CREATE FUNCTION catalog_ontology_counter() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        UPDATE sbol_catalog_stats SET ontologies = ontologies + 1 WHERE singleton;
        RETURN NEW;
    END IF;
    UPDATE sbol_catalog_stats SET ontologies = ontologies - 1 WHERE singleton;
    RETURN OLD;
END;
$$;
CREATE TRIGGER catalog_ontology_counter_insert
AFTER INSERT ON sbol_ontologies FOR EACH ROW EXECUTE FUNCTION catalog_ontology_counter();
CREATE TRIGGER catalog_ontology_counter_delete
AFTER DELETE ON sbol_ontologies FOR EACH ROW EXECUTE FUNCTION catalog_ontology_counter();

-- Accelerator rows are graph-owned derived state. This also makes graph
-- deletion exercise the resource/sequence counter triggers above.
CREATE FUNCTION catalog_delete_graph_projection() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM accel_dirty WHERE graph_iri = OLD.iri;
    DELETE FROM accel_member WHERE graph_iri = OLD.iri;
    DELETE FROM accel_facet WHERE graph_iri = OLD.iri;
    DELETE FROM accel_role WHERE graph_iri = OLD.iri;
    DELETE FROM accel_type WHERE graph_iri = OLD.iri;
    DELETE FROM accel_object WHERE graph_iri = OLD.iri;
    RETURN OLD;
END;
$$;
CREATE TRIGGER catalog_delete_graph_projection
BEFORE DELETE ON sbol_graphs FOR EACH ROW EXECUTE FUNCTION catalog_delete_graph_projection();

-- TRUNCATE does not fire row-level DELETE triggers. Keep the projection exact
-- for administrative resets and isolated test databases as well as ordinary
-- application writes. PostgreSQL runs AFTER TRUNCATE triggers after every
-- table in a cascaded truncate has been emptied.
CREATE FUNCTION catalog_reset_after_truncate() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE sbol_graphs g
    SET triple_count = (SELECT count(*) FROM sbol_triples t WHERE t.graph_iri = g.iri),
        resource_count = (SELECT count(*) FROM accel_object o WHERE o.graph_iri = g.iri);
    UPDATE sbol_catalog_stats
    SET resources = (SELECT count(DISTINCT iri) FROM accel_object),
        named_graphs = (SELECT count(*) FROM sbol_graphs),
        triples = (SELECT count(*) FROM sbol_triples),
        sequences = (SELECT count(DISTINCT iri) FROM accel_type
          WHERE type_iri IN ('http://sbols.org/v2#Sequence', 'http://sbols.org/v3#Sequence')),
        ontologies = (SELECT count(*) FROM sbol_ontologies)
    WHERE singleton;
    RETURN NULL;
END;
$$;
CREATE TRIGGER catalog_reset_graphs_after_truncate
AFTER TRUNCATE ON sbol_graphs FOR EACH STATEMENT EXECUTE FUNCTION catalog_reset_after_truncate();
CREATE TRIGGER catalog_reset_triples_after_truncate
AFTER TRUNCATE ON sbol_triples FOR EACH STATEMENT EXECUTE FUNCTION catalog_reset_after_truncate();
CREATE TRIGGER catalog_reset_resources_after_truncate
AFTER TRUNCATE ON accel_object FOR EACH STATEMENT EXECUTE FUNCTION catalog_reset_after_truncate();
CREATE TRIGGER catalog_reset_types_after_truncate
AFTER TRUNCATE ON accel_type FOR EACH STATEMENT EXECUTE FUNCTION catalog_reset_after_truncate();
CREATE TRIGGER catalog_reset_ontologies_after_truncate
AFTER TRUNCATE ON sbol_ontologies FOR EACH STATEMENT EXECUTE FUNCTION catalog_reset_after_truncate();
