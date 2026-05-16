//! Persistence for the typed SBOL projections. Each table
//! shares the IRI-keyed upsert shape. Rows are inserted *after* the
//! corresponding `sbol_objects` row in the same transaction; the
//! `object_id` is looked up via the IRI to keep these methods independent
//! of the upsert order.

use sbol_db_core::{
    ComponentProjection, ConstraintProjection, DomainError, FeatureProjection,
    InteractionProjection, IriString, LocationProjection, ParticipationProjection,
    SequenceProjection, TypedProjections,
};

use crate::repo::db_err;
use crate::PgPool;

#[derive(Clone)]
pub struct TypedProjectionRepository {
    _pool: PgPool,
}

impl TypedProjectionRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { _pool: pool }
    }

    /// Upsert every typed projection produced for a single document. Run
    /// inside the import transaction.
    pub async fn upsert_all(
        &self,
        conn: &mut sqlx::PgConnection,
        projections: &TypedProjections,
    ) -> Result<TypedProjectionCounts, DomainError> {
        let mut counts = TypedProjectionCounts::default();
        for c in &projections.components {
            upsert_component(conn, c).await?;
            counts.components += 1;
        }
        for s in &projections.sequences {
            upsert_sequence(conn, s).await?;
            counts.sequences += 1;
        }
        for f in &projections.features {
            upsert_feature(conn, f).await?;
            counts.features += 1;
        }
        for l in &projections.locations {
            upsert_location(conn, l).await?;
            counts.locations += 1;
        }
        for c in &projections.constraints {
            upsert_constraint(conn, c).await?;
            counts.constraints += 1;
        }
        for i in &projections.interactions {
            upsert_interaction(conn, i).await?;
            counts.interactions += 1;
        }
        for p in &projections.participations {
            upsert_participation(conn, p).await?;
            counts.participations += 1;
        }
        Ok(counts)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TypedProjectionCounts {
    pub components: usize,
    pub sequences: usize,
    pub features: usize,
    pub locations: usize,
    pub constraints: usize,
    pub interactions: usize,
    pub participations: usize,
}

fn as_str_opt(value: &Option<IriString>) -> Option<&str> {
    value.as_ref().map(|i| i.as_str())
}

async fn upsert_component(
    conn: &mut sqlx::PgConnection,
    p: &ComponentProjection,
) -> Result<(), DomainError> {
    sqlx::query(
        r#"
        INSERT INTO sbol_components (
            object_id, iri, types, roles, sequence_iris, feature_iris,
            interaction_iris, model_iris
        )
        SELECT id, $1,
               $2::text[]::sbol_ontology_term[],
               $3::text[]::sbol_ontology_term[],
               $4::text[]::sbol_iri[],
               $5::text[]::sbol_iri[],
               $6::text[]::sbol_iri[],
               $7::text[]::sbol_iri[]
        FROM sbol_objects WHERE iri = $1
        ON CONFLICT (object_id) DO UPDATE SET
            types = EXCLUDED.types,
            roles = EXCLUDED.roles,
            sequence_iris = EXCLUDED.sequence_iris,
            feature_iris = EXCLUDED.feature_iris,
            interaction_iris = EXCLUDED.interaction_iris,
            model_iris = EXCLUDED.model_iris,
            updated_at = now()
        "#,
    )
    .bind(p.iri.as_str())
    .bind(&p.types)
    .bind(&p.roles)
    .bind(&p.sequence_iris)
    .bind(&p.feature_iris)
    .bind(&p.interaction_iris)
    .bind(&p.model_iris)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(())
}

async fn upsert_sequence(
    conn: &mut sqlx::PgConnection,
    p: &SequenceProjection,
) -> Result<(), DomainError> {
    let alphabet_str = p.alphabet.map(|a| a.as_db_str());
    let row = sqlx::query(
        r#"
        INSERT INTO sbol_sequences (
            object_id, iri, encoding_iri, elements, alphabet, content_hash
        )
        SELECT id, $1, $2, $3, $4, $5
        FROM sbol_objects WHERE iri = $1
        ON CONFLICT (object_id) DO UPDATE SET
            encoding_iri = EXCLUDED.encoding_iri,
            elements = EXCLUDED.elements,
            alphabet = EXCLUDED.alphabet,
            content_hash = EXCLUDED.content_hash
        RETURNING object_id
        "#,
    )
    .bind(p.iri.as_str())
    .bind(as_str_opt(&p.encoding_iri))
    .bind(p.elements.as_deref())
    .bind(alphabet_str)
    .bind(p.content_hash.as_deref())
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_err)?;
    if let Some(row) = row {
        use sqlx::Row;
        let object_id: uuid::Uuid = row.try_get("object_id").map_err(db_err)?;
        crate::repo::sequence_search::reindex_kmers(
            conn,
            object_id,
            p.elements.as_deref(),
            alphabet_str,
        )
        .await?;
    }
    Ok(())
}

async fn upsert_feature(
    conn: &mut sqlx::PgConnection,
    p: &FeatureProjection,
) -> Result<(), DomainError> {
    sqlx::query(
        r#"
        INSERT INTO sbol_features (
            object_id, iri, parent_component_iri, feature_kind,
            instance_of_iri, roles, orientation_iri
        )
        SELECT id, $1, $2, $3, $4, $5::text[]::sbol_ontology_term[], $6
        FROM sbol_objects WHERE iri = $1
        ON CONFLICT (object_id) DO UPDATE SET
            parent_component_iri = EXCLUDED.parent_component_iri,
            feature_kind = EXCLUDED.feature_kind,
            instance_of_iri = EXCLUDED.instance_of_iri,
            roles = EXCLUDED.roles,
            orientation_iri = EXCLUDED.orientation_iri
        "#,
    )
    .bind(p.iri.as_str())
    .bind(as_str_opt(&p.parent_component_iri))
    .bind(&p.feature_kind)
    .bind(as_str_opt(&p.instance_of_iri))
    .bind(&p.roles)
    .bind(as_str_opt(&p.orientation_iri))
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(())
}

async fn upsert_location(
    conn: &mut sqlx::PgConnection,
    p: &LocationProjection,
) -> Result<(), DomainError> {
    sqlx::query(
        r#"
        INSERT INTO sbol_locations (
            object_id, iri, feature_iri, sequence_iri, location_kind,
            start_pos, end_pos, cut_pos, orientation_iri, data
        )
        SELECT id, $1, $2, $3, $4, $5, $6, $7, $8, $9
        FROM sbol_objects WHERE iri = $1
        ON CONFLICT (object_id) DO UPDATE SET
            feature_iri = EXCLUDED.feature_iri,
            sequence_iri = EXCLUDED.sequence_iri,
            location_kind = EXCLUDED.location_kind,
            start_pos = EXCLUDED.start_pos,
            end_pos = EXCLUDED.end_pos,
            cut_pos = EXCLUDED.cut_pos,
            orientation_iri = EXCLUDED.orientation_iri,
            data = EXCLUDED.data
        "#,
    )
    .bind(p.iri.as_str())
    .bind(as_str_opt(&p.feature_iri))
    .bind(as_str_opt(&p.sequence_iri))
    .bind(&p.location_kind)
    .bind(p.start_pos)
    .bind(p.end_pos)
    .bind(p.cut_pos)
    .bind(as_str_opt(&p.orientation_iri))
    .bind(&p.data)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(())
}

async fn upsert_constraint(
    conn: &mut sqlx::PgConnection,
    p: &ConstraintProjection,
) -> Result<(), DomainError> {
    sqlx::query(
        r#"
        INSERT INTO sbol_constraints (
            object_id, iri, parent_component_iri, restriction_iri,
            subject_iri, object_iri
        )
        SELECT id, $1, $2, $3, $4, $5
        FROM sbol_objects WHERE iri = $1
        ON CONFLICT (object_id) DO UPDATE SET
            parent_component_iri = EXCLUDED.parent_component_iri,
            restriction_iri = EXCLUDED.restriction_iri,
            subject_iri = EXCLUDED.subject_iri,
            object_iri = EXCLUDED.object_iri
        "#,
    )
    .bind(p.iri.as_str())
    .bind(as_str_opt(&p.parent_component_iri))
    .bind(as_str_opt(&p.restriction_iri))
    .bind(as_str_opt(&p.subject_iri))
    .bind(as_str_opt(&p.object_iri))
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(())
}

async fn upsert_interaction(
    conn: &mut sqlx::PgConnection,
    p: &InteractionProjection,
) -> Result<(), DomainError> {
    sqlx::query(
        r#"
        INSERT INTO sbol_interactions (
            object_id, iri, parent_component_iri, interaction_types
        )
        SELECT id, $1, $2, $3::text[]::sbol_ontology_term[]
        FROM sbol_objects WHERE iri = $1
        ON CONFLICT (object_id) DO UPDATE SET
            parent_component_iri = EXCLUDED.parent_component_iri,
            interaction_types = EXCLUDED.interaction_types
        "#,
    )
    .bind(p.iri.as_str())
    .bind(as_str_opt(&p.parent_component_iri))
    .bind(&p.interaction_types)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(())
}

async fn upsert_participation(
    conn: &mut sqlx::PgConnection,
    p: &ParticipationProjection,
) -> Result<(), DomainError> {
    sqlx::query(
        r#"
        INSERT INTO sbol_participations (
            object_id, iri, interaction_iri, participant_iri, roles
        )
        SELECT id, $1, $2, $3, $4::text[]::sbol_ontology_term[]
        FROM sbol_objects WHERE iri = $1
        ON CONFLICT (object_id) DO UPDATE SET
            interaction_iri = EXCLUDED.interaction_iri,
            participant_iri = EXCLUDED.participant_iri,
            roles = EXCLUDED.roles
        "#,
    )
    .bind(p.iri.as_str())
    .bind(as_str_opt(&p.interaction_iri))
    .bind(as_str_opt(&p.participant_iri))
    .bind(&p.roles)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(())
}
