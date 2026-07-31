//! The V2 objects resource.
//!
//! One idiomatic path, `/api/v2/objects/{iri}`, carries the object's whole
//! lifecycle under proper HTTP verbs: `GET` reads it (JSON metadata, an RDF
//! closure by `Accept`, or a download by `?format=`), `PATCH` edits its mutable
//! fields, and `DELETE` removes it. `GET /api/v2/objects` lists objects with
//! real pagination. The object IRI is a single percent-encoded path segment
//! (idiomatic REST) rather than the V1 path grammar. Visibility comes from the
//! caller's [`GraphScope`]: an object in a graph the caller is not entitled to
//! read is reported as `404`, never disclosed. Every handler delegates to the
//! same facade verbs the V1 adapter calls; it holds no business logic.

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::header::{CONTENT_TYPE, LOCATION};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use sbol_db_app::{EditService, FieldValue, MakePublicRequest, MutationService};
use serde::Deserialize;
use serde_json::json;

use super::auth::{require_user, scope_for, Identity};
use super::download::{render_download, DownloadFormat};
use super::negotiate::{content_type_for, negotiate, Negotiated};
use super::search::{run_search, SearchParams, SearchResponse};
use super::util::{encode_iri_segment, parse_json, resolve_overwrite};
use crate::error::ApiError;
use crate::export;
use crate::v2::error::V2Error;
use crate::AppState;

/// The mutable metadata predicates a `PATCH` writes.
const DCTERMS_TITLE: &str = "http://purl.org/dc/terms/title";
const DCTERMS_DESCRIPTION: &str = "http://purl.org/dc/terms/description";

/// `GET /api/v2/objects` — the paginated, ACL-scoped object list. Shares the
/// ranked query and response envelope with `/search`; `?type=` and `?q=` narrow
/// it, `?limit=`/`?offset=` page it.
pub async fn list_objects(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(params): Query<SearchParams>,
) -> Result<Json<SearchResponse>, V2Error> {
    Ok(Json(run_search(&state, &identity, params).await?))
}

/// The read selectors on `GET /api/v2/objects/{iri}`: an explicit download
/// `format`, and the SBOL `version` for the RDF-bearing representations.
#[derive(Debug, Default, Deserialize)]
pub struct GetObjectParams {
    /// A download format (`sbol`, `sbolnr`, `gb`, `fasta`, `gff`, `omex`). When
    /// absent, the representation is chosen by `Accept` negotiation.
    pub format: Option<String>,
    /// `sbol2` or `sbol3` (default `sbol3`) for the RDF-bearing formats; the
    /// sequence formats ignore it.
    pub version: Option<String>,
}

/// `GET /api/v2/objects/{iri}` — one object under the caller's authorized graph
/// scope. An explicit `?format=` is a closure download; otherwise `Accept`
/// selects idiomatic JSON metadata or the object's RDF closure.
pub async fn get_object(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(iri): Path<String>,
    Query(params): Query<GetObjectParams>,
    headers: axum::http::HeaderMap,
) -> Result<Response, V2Error> {
    let requested_scope = scope_for(&state, &identity).await?;
    // Resolve visibility across every graph carrying this subject. Imported
    // objects commonly occur in both a source graph and the public projection;
    // selecting an arbitrary first graph would incorrectly hide the public
    // copy. The returned scope also expands authorized logical document graphs
    // to their physical storage graph for the closure crawl below.
    let scope = state
        .app
        .object_read_scope(&iri, requested_scope)
        .await?
        .ok_or_else(|| not_found(&iri))?;

    let sbol2 = wants_sbol2(params.version.as_deref())?;

    // An explicit ?format= is a download of the object's closure; otherwise the
    // representation is negotiated from Accept. The download and RDF paths read
    // the triplestore directly, so they serve verbatim submissions that carry no
    // derived object record; only the JSON metadata path needs the record.
    if let Some(format) = params.format.as_deref() {
        let format = DownloadFormat::parse(format).ok_or_else(|| {
            V2Error::from(ApiError::BadRequest(format!(
                "unsupported format: {format}"
            )))
        })?;
        let display_id = state
            .service
            .get_object_by_iri(&iri)
            .await
            .map_err(V2Error::from)?
            .and_then(|r| r.display_id)
            .unwrap_or_else(|| last_segment(&iri));
        return render_download(&state, &iri, &display_id, format, sbol2, scope).await;
    }

    match negotiate(&headers)? {
        Negotiated::Json => {
            let record = state
                .service
                .get_object_by_iri(&iri)
                .await
                .map_err(V2Error::from)?
                .ok_or_else(|| not_found(&iri))?;
            Ok(Json(record).into_response())
        }
        Negotiated::Rdf(format) => {
            let body = export::export_subject_rdf(state.service.as_ref(), &iri, format, sbol2)
                .await
                .map_err(V2Error::from)?;
            Ok((
                [(CONTENT_TYPE, content_type_for(Negotiated::Rdf(format)))],
                body,
            )
                .into_response())
        }
    }
}

/// `GET /api/v2/objects/{iri}/details` — the normalized, ACL-scoped object-page
/// resource. The application facade owns the biological relationships and
/// explicit availability states; this handler only supplies identity scope and
/// the V2 wire envelope. Unknown and out-of-scope objects are both `404`.
pub async fn get_object_details(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(iri): Path<String>,
) -> Result<Response, V2Error> {
    let scope = scope_for(&state, &identity).await?;
    let details = state
        .app
        .object_details(&iri, scope)
        .await?
        .ok_or_else(|| not_found(&iri))?;
    Ok(Json(details).into_response())
}

/// A `PATCH /api/v2/objects/{iri}` body: the mutable fields a caller may edit
/// in place. Every field is optional; a present field is applied through the
/// matching [`EditService`] verb, an absent one is left untouched.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PatchBody {
    /// The object title (`dcterms:title`).
    pub name: Option<String>,
    /// The object description (`dcterms:description`).
    pub description: Option<String>,
    /// The mutable rich-text description (`sbh:mutableDescription`).
    pub mutable_description: Option<String>,
    /// The mutable notes (`sbh:mutableNotes`).
    pub mutable_notes: Option<String>,
    /// The mutable provenance/source (`sbh:mutableProvenance`).
    pub mutable_source: Option<String>,
    /// The full PubMed citation list (`obo:OBI_0001617`), replacing the current
    /// one when present.
    pub citations: Option<Vec<String>>,
}

/// `PATCH /api/v2/objects/{iri}` — edit the object's mutable fields in place.
/// Identity-gated through the facade: an anonymous caller is `403`, a non-owner
/// (or a non-admin editing a public object) is rejected by the facade.
pub async fn patch_object(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(iri): Path<String>,
    body: Bytes,
) -> Result<Response, V2Error> {
    let user = require_user(&identity)?;
    let patch: PatchBody = parse_json(&body)?;
    let svc = edit_service(&state);

    if let Some(name) = &patch.name {
        svc.edit_field(
            &user.graph_uri,
            user.is_admin,
            &iri,
            DCTERMS_TITLE,
            &FieldValue::Literal(name.clone()),
            None,
        )
        .await?;
    }
    if let Some(description) = &patch.description {
        svc.edit_field(
            &user.graph_uri,
            user.is_admin,
            &iri,
            DCTERMS_DESCRIPTION,
            &FieldValue::Literal(description.clone()),
            None,
        )
        .await?;
    }
    if let Some(value) = &patch.mutable_description {
        svc.update_mutable_description(&user.graph_uri, user.is_admin, &iri, value)
            .await?;
    }
    if let Some(value) = &patch.mutable_notes {
        svc.update_mutable_notes(&user.graph_uri, user.is_admin, &iri, value)
            .await?;
    }
    if let Some(value) = &patch.mutable_source {
        svc.update_mutable_source(&user.graph_uri, user.is_admin, &iri, value)
            .await?;
    }
    if let Some(citations) = &patch.citations {
        svc.update_citations(&user.graph_uri, user.is_admin, &iri, citations)
            .await?;
    }

    // Return the edited object so the caller sees the applied state. A verbatim
    // submission carries no derived object record; there the edited triples are
    // authoritative, so the response echoes the object IRI rather than a
    // misleading 404.
    match state
        .service
        .get_object_by_iri(&iri)
        .await
        .map_err(V2Error::from)?
    {
        Some(record) => Ok(Json(record).into_response()),
        None => Ok(Json(json!({ "iri": iri })).into_response()),
    }
}

/// `DELETE /api/v2/objects/{iri}` — remove a top-level object and everything
/// whose `sbh:topLevel` names it. Identity-gated through the facade; a missing
/// object is `404`, an unauthorized caller is `403`. Returns `204 No Content`.
pub async fn delete_object(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(iri): Path<String>,
) -> Result<StatusCode, V2Error> {
    let user = require_user(&identity)?;
    mutation_service(&state)
        .remove(&user.graph_uri, user.is_admin, &iri)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A `POST /api/v2/objects/{iri}/publish` body: the target public identity plus
/// the metadata stamped onto the new public root collection.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PublishBody {
    /// The public submission id (required).
    pub id: Option<String>,
    /// The public version (required).
    pub version: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    /// PubMed citation ids stamped on the public collection.
    pub citations: Vec<String>,
    /// Collision policy against the public collection URI (`fail`, `replace`,
    /// `merge`); defaults to `fail`.
    pub overwrite: Option<String>,
}

/// `POST /api/v2/objects/{iri}/publish` — publish a private object to the public
/// graph under freshly minted public URIs (classic makePublic). Identity-gated
/// through the facade. Returns `201 Created` with a `Location` to the public
/// collection.
pub async fn publish_object(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(iri): Path<String>,
    body: Bytes,
) -> Result<Response, V2Error> {
    let user = require_user(&identity)?;
    let form: PublishBody = parse_json(&body)?;
    let public_id = super::util::required(form.id, "id")?;
    let version = super::util::required(form.version, "version")?;

    let request = MakePublicRequest {
        source_uri: iri.clone(),
        owner_username: user.username.clone(),
        public_id,
        version,
        name: form.name.filter(|s| !s.is_empty()),
        description: form.description.filter(|s| !s.is_empty()),
        creator_name: Some(user.name.clone()),
        citations: form.citations,
        overwrite: resolve_overwrite(form.overwrite.as_deref())?,
    };

    let outcome = mutation_service(&state)
        .make_public(&user.graph_uri, user.is_admin, request)
        .await?;

    let members: Vec<&str> = outcome.members.iter().map(|m| m.as_str()).collect();
    let payload = json!({
        "collection_uri": outcome.collection_uri.as_str(),
        "members": members,
        "triple_count": outcome.triple_count,
    });
    let location = format!(
        "/api/v2/objects/{}",
        encode_iri_segment(outcome.collection_uri.as_str())
    );
    Ok((StatusCode::CREATED, [(LOCATION, location)], Json(payload)).into_response())
}

/// Build the [`EditService`] from the shared facade handles, the same wiring the
/// V1 edit routes use.
fn edit_service(state: &AppState) -> EditService {
    state.app.edit_service()
}

/// Build the [`MutationService`] from the shared facade handles, the same wiring
/// the V1 mutate routes use.
fn mutation_service(state: &AppState) -> MutationService {
    state.app.mutation_service()
}

/// A non-disclosing `404` for an absent or out-of-scope object.
fn not_found(iri: &str) -> V2Error {
    V2Error::from(ApiError::NotFound(format!("object {iri}")))
}

/// Resolve the `?version=` selector to whether the caller asked for SBOL2
/// output. Absent or `sbol3` keeps the native SBOL3 view.
fn wants_sbol2(version: Option<&str>) -> Result<bool, V2Error> {
    match version.unwrap_or("sbol3") {
        "sbol3" | "3" => Ok(false),
        "sbol2" | "2" => Ok(true),
        other => Err(V2Error::from(ApiError::BadRequest(format!(
            "unknown version: {other}"
        )))),
    }
}

/// The last path segment of an IRI, the display-id fallback for a download
/// filename when the object record carries none.
fn last_segment(iri: &str) -> String {
    iri.rsplit(['/', '#']).next().unwrap_or(iri).to_owned()
}
