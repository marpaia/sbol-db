-- Root-collection queries start from an rdf:type scan and exclude any object
-- referenced by sbol2:member. The original membership index is ordered by
-- collection, so add the reverse lookup needed by that anti-join.
CREATE INDEX accel_member_member_idx
    ON accel_member (graph_iri, member_iri);
