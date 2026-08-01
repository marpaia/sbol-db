//! SynBioHub v1 share-link surface.
//!
//! An object owner mints a share hash with `<object>/shareLink`; anyone holding
//! the hash reads the (possibly private) object through
//! `/user/<u>/<c>/<d>/<v>/<hash>/share/…` without an account. The hash is
//! `sha1('synbiohub_' + sha1(uri) + salt)` (see [`sbol_db_app::share_hash`]), so
//! a valid hash proves authorization for exactly that object; the read then runs
//! under [`GraphScope::Union`] because the grant is object-scoped and every
//! handler targets the object's own closure.

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use sbol_db_sparql::GraphScope;
use serde::Deserialize;
use serde_json::json;

use super::download::{download_scoped, DownloadParams, Format};
use super::routes::{run_scoped_value, UserObject};
use super::{queries, render};
use crate::{ApiError, AppState};

/// A share-scoped object path: the object identity plus the share `:hash`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareObject {
    pub user_id: String,
    pub collection_id: String,
    pub display_id: String,
    pub version: String,
    pub hash: String,
}

impl ShareObject {
    fn object_uri(&self, state: &AppState) -> String {
        format!(
            "{}user/{}/{}/{}/{}",
            state.app.registry_namespace.database_prefix(),
            self.user_id,
            self.collection_id,
            self.display_id,
            self.version
        )
    }
}

/// Verify the share hash for the object, returning the object URI on success.
/// A mismatched hash is a `404`, revealing nothing about the object's existence.
fn verify(state: &AppState, object: &ShareObject) -> Result<String, ApiError> {
    let uri = object.object_uri(state);
    let expected = sbol_db_app::share_hash(&uri, &state.config.share_link_salt);
    if expected == object.hash {
        Ok(uri)
    } else {
        Err(ApiError::NotFound(format!("share link for {uri}")))
    }
}

/// `GET <object>/shareLink` — mint the object's share hash and URL. Only the
/// owner reaches this (the router applies the identity gate), matching classic.
pub async fn share_link(State(state): State<AppState>, Path(object): Path<UserObject>) -> Response {
    let uri = super::routes::user_uri(&state, &object);
    let hash = sbol_db_app::share_hash(&uri, &state.config.share_link_salt);
    let url = format!(
        "/user/{}/{}/{}/{}/{}/share",
        object.user_id, object.collection_id, object.display_id, object.version, hash
    );
    Json(json!({ "hash": hash, "url": url })).into_response()
}

/// Generate a share download handler for one [`Format`]: verify the hash, then
/// serve the object's closure under `Union` scope.
macro_rules! share_download {
    ($name:ident, $format:expr) => {
        pub async fn $name(
            State(state): State<AppState>,
            Path(object): Path<ShareObject>,
            Query(params): Query<DownloadParams>,
        ) -> Result<Response, ApiError> {
            let uri = verify(&state, &object)?;
            download_scoped(
                state,
                GraphScope::Union,
                uri,
                object.display_id,
                $format,
                params.version,
            )
            .await
        }
    };
}

// The bare share view and `/full` serve the recursive SBOL closure, like the
// bare object route; the format routes mirror the download surface.
share_download!(share_bare, Format::Sbol);
share_download!(share_full, Format::Sbol);
share_download!(share_sbol, Format::Sbol);
share_download!(share_sbolnr, Format::SbolNonRecursive);
share_download!(share_genbank, Format::GenBank);
share_download!(share_fasta, Format::Fasta);
share_download!(share_gff, Format::Gff3);
share_download!(share_omex, Format::Omex);
share_download!(share_summary, Format::Summary);

/// `GET .../share/metadata` — the object's metadata rows.
pub async fn share_metadata(
    State(state): State<AppState>,
    Path(object): Path<ShareObject>,
) -> Result<Response, ApiError> {
    let uri = verify(&state, &object)?;
    let results = run_scoped_value(&state, &queries::metadata(&uri), GraphScope::Union).await?;
    Ok(render::metadata_response(&results))
}

/// `GET .../share/subCollections`.
pub async fn share_sub_collections(
    State(state): State<AppState>,
    Path(object): Path<ShareObject>,
) -> Result<Response, ApiError> {
    let uri = verify(&state, &object)?;
    let results =
        run_scoped_value(&state, &queries::sub_collections(&uri), GraphScope::Union).await?;
    Ok(render::collections_response(&results))
}

/// Generate a share relation handler (`uses`/`twins`, listing or count).
macro_rules! share_relation {
    ($name:ident, $query:path, $count:expr) => {
        pub async fn $name(
            State(state): State<AppState>,
            Path(object): Path<ShareObject>,
        ) -> Result<Response, ApiError> {
            let uri = verify(&state, &object)?;
            let results =
                run_scoped_value(&state, &$query(&uri, $count), GraphScope::Union).await?;
            Ok(if $count {
                render::count_response(&results)
            } else {
                render::search_response(&results)
            })
        }
    };
}

share_relation!(share_uses, queries::uses, false);
share_relation!(share_uses_count, queries::uses, true);
share_relation!(share_twins, queries::twins, false);
share_relation!(share_twins_count, queries::twins, true);
