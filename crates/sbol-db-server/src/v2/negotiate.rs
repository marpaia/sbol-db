//! `Accept`-header content negotiation.
//!
//! [`negotiate`] maps a request's `Accept` header to the representation a V2
//! read handler should return: idiomatic JSON, or one of the RDF
//! serializations the P3 serializers already produce. Absent or `*/*` yields
//! JSON; an `Accept` that lists only unsupported media types is a `406 Not
//! Acceptable`, reusing the same status the original SBOL DB SPARQL surface returns for
//! an unsupported result format.

use axum::http::header::ACCEPT;
use axum::http::HeaderMap;
use sbol_db_core::SerializationFormat;

use crate::error::ApiError;
use crate::v2::error::V2Error;

/// The representation a caller asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Negotiated {
    /// The resource as idiomatic JSON.
    Json,
    /// The resource's RDF closure in the given serialization.
    Rdf(SerializationFormat),
}

/// Resolve one `Accept` media type to a [`Negotiated`], or `None` when it is a
/// type this surface does not serve.
fn media_to_negotiated(media: &str) -> Option<Negotiated> {
    match media {
        "*/*" | "application/*" | "application/json" => Some(Negotiated::Json),
        "text/turtle" | "application/x-turtle" => {
            Some(Negotiated::Rdf(SerializationFormat::Turtle))
        }
        "application/rdf+xml" => Some(Negotiated::Rdf(SerializationFormat::RdfXml)),
        "application/ld+json" => Some(Negotiated::Rdf(SerializationFormat::JsonLd)),
        "application/n-triples" => Some(Negotiated::Rdf(SerializationFormat::NTriples)),
        _ => None,
    }
}

/// The `Content-Type` to write for a negotiated representation.
pub fn content_type_for(negotiated: Negotiated) -> &'static str {
    match negotiated {
        Negotiated::Json => "application/json",
        Negotiated::Rdf(SerializationFormat::Turtle) => "text/turtle",
        Negotiated::Rdf(SerializationFormat::RdfXml) => "application/rdf+xml",
        Negotiated::Rdf(SerializationFormat::JsonLd) => "application/ld+json",
        Negotiated::Rdf(SerializationFormat::NTriples) => "application/n-triples",
        // No other RDF format is produced by [`media_to_negotiated`].
        Negotiated::Rdf(_) => "application/octet-stream",
    }
}

/// Negotiate the response representation from `Accept`. An absent or blank
/// header defaults to JSON; otherwise the header's media ranges are tried in
/// descending q-value order and the first supported one wins. A header naming
/// only unsupported types is a `406`.
pub fn negotiate(headers: &HeaderMap) -> Result<Negotiated, V2Error> {
    let accept = headers
        .get(ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim();
    if accept.is_empty() {
        return Ok(Negotiated::Json);
    }

    // Parse `media;q=<weight>` ranges, defaulting weight to 1.0, and keep the
    // original order to break q-value ties deterministically (first-listed
    // wins, as browsers expect).
    let mut ranges: Vec<(usize, &str, f32)> = accept
        .split(',')
        .enumerate()
        .filter_map(|(index, raw)| {
            let mut parts = raw.split(';');
            let media = parts.next()?.trim();
            if media.is_empty() {
                return None;
            }
            let q = parts
                .find_map(|p| {
                    let p = p.trim();
                    p.strip_prefix("q=")
                        .and_then(|v| v.trim().parse::<f32>().ok())
                })
                .unwrap_or(1.0);
            Some((index, media, q))
        })
        .collect();
    // Stable sort by descending q; equal q preserves listed order.
    ranges.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.0.cmp(&b.0)));

    for (_, media, _) in &ranges {
        if let Some(negotiated) = media_to_negotiated(media) {
            return Ok(negotiated);
        }
    }
    Err(
        ApiError::Sparql(sbol_db_sparql::SparqlError::UnsupportedFormat(
            accept.to_owned(),
        ))
        .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header::HeaderValue;

    fn headers(accept: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(ACCEPT, HeaderValue::from_str(accept).unwrap());
        h
    }

    #[test]
    fn json_default_when_absent() {
        assert_eq!(negotiate(&HeaderMap::new()).unwrap(), Negotiated::Json);
    }

    #[test]
    fn wildcard_is_json() {
        assert_eq!(negotiate(&headers("*/*")).unwrap(), Negotiated::Json);
    }

    #[test]
    fn turtle_and_rdfxml() {
        assert_eq!(
            negotiate(&headers("text/turtle")).unwrap(),
            Negotiated::Rdf(SerializationFormat::Turtle)
        );
        assert_eq!(
            negotiate(&headers("application/rdf+xml")).unwrap(),
            Negotiated::Rdf(SerializationFormat::RdfXml)
        );
    }

    #[test]
    fn qvalue_prefers_higher_weight() {
        // Turtle is listed first but JSON has the higher q, so JSON wins.
        let chosen = negotiate(&headers("text/turtle;q=0.5, application/json;q=0.9")).unwrap();
        assert_eq!(chosen, Negotiated::Json);
    }

    #[test]
    fn unsupported_is_406() {
        use axum::response::IntoResponse;
        let err = negotiate(&headers("image/png")).unwrap_err();
        assert_eq!(
            err.into_response().status(),
            axum::http::StatusCode::NOT_ACCEPTABLE
        );
    }

    #[test]
    fn content_type_round_trips() {
        assert_eq!(content_type_for(Negotiated::Json), "application/json");
        assert_eq!(
            content_type_for(Negotiated::Rdf(SerializationFormat::Turtle)),
            "text/turtle"
        );
    }
}
