ALTER TABLE sbol_documents
    DROP CONSTRAINT sbol_documents_serialization_format_check;

ALTER TABLE sbol_documents
    ADD CONSTRAINT sbol_documents_serialization_format_check
    CHECK (serialization_format IN (
        'json',
        'jsonld',
        'rdfxml',
        'turtle',
        'trig',
        'ntriples',
        'nquads',
        'genbank',
        'fasta'
    ));
