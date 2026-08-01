//! Permission-aware MCP resources over the same application facade as tools.

use serde::Serialize;
use serde_json::{json, Value};
use url::Url;

use super::{
    serialize_download, tool_error, AppState, DispatchError, DownloadFormat, RequestPrincipal,
    MAX_EMBEDDED_DOWNLOAD_BYTES, SCOPE_READ, SCOPE_REVIEW,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResourceDefinition {
    uri: &'static str,
    name: &'static str,
    title: &'static str,
    description: &'static str,
    mime_type: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResourceTemplateDefinition {
    uri_template: &'static str,
    name: &'static str,
    title: &'static str,
    description: &'static str,
    mime_type: &'static str,
}

pub(super) fn list(principal: &RequestPrincipal) -> Value {
    let mut resources = vec![
        ResourceDefinition {
            uri: "sbol://registry",
            name: "registry",
            title: "SBOL registry connection",
            description: "The connected registry, API and agent-access endpoints, and delegated capability context.",
            mime_type: "application/json",
        },
        ResourceDefinition {
            uri: "sbol://account",
            name: "account",
            title: "Signed-in SBOL account",
            description: "The account whose ownership and sharing permissions scope every resource and tool call.",
            mime_type: "application/json",
        },
    ];
    if principal.has_scopes(&[SCOPE_READ, SCOPE_REVIEW]) {
        resources.push(ResourceDefinition {
            uri: "sbol://reviews",
            name: "reviews",
            title: "Design review queue",
            description: "Review cases requested by or assigned to the signed-in account.",
            mime_type: "application/json",
        });
    }
    json!({ "resources": resources })
}

pub(super) fn templates() -> Value {
    json!({
        "resourceTemplates": [
            ResourceTemplateDefinition {
                uri_template: "sbol://design{?iri}",
                name: "design",
                title: "Complete visible design record",
                description: "Normalized biological, provenance, sequence, collection, and collaboration context for a canonical design IRI.",
                mime_type: "application/json",
            },
            ResourceTemplateDefinition {
                uri_template: "sbol://design-content{?iri,format}",
                name: "design-content",
                title: "Serialized SBOL design",
                description: "The ACL-scoped design closure in Turtle, RDF/XML, JSON-LD, or N-Triples. Format defaults to turtle.",
                mime_type: "text/turtle",
            }
        ]
    })
}

pub(super) fn required_scopes(params: &Value) -> &'static [&'static str] {
    params
        .get("uri")
        .and_then(Value::as_str)
        .and_then(|uri| Url::parse(uri).ok())
        .and_then(|uri| (uri.scheme() == "sbol").then(|| uri.host_str().map(str::to_owned)))
        .flatten()
        .filter(|host| host == "reviews")
        .map(|_| &[SCOPE_READ, SCOPE_REVIEW][..])
        .unwrap_or(&[SCOPE_READ])
}

pub(super) async fn read(
    state: &AppState,
    principal: &RequestPrincipal,
    params: &Value,
) -> Result<Value, DispatchError> {
    let requested = params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| DispatchError::Protocol("resources/read requires a uri".to_owned()))?;
    let uri = Url::parse(requested)
        .map_err(|error| DispatchError::Protocol(format!("invalid resource URI: {error}")))?;
    if uri.scheme() != "sbol" {
        return Err(DispatchError::Protocol(
            "SBOL DB resource URIs use the sbol scheme".to_owned(),
        ));
    }
    let host = uri
        .host_str()
        .ok_or_else(|| DispatchError::Protocol("resource URI has no kind".to_owned()))?;
    let user = principal
        .authenticated_user()
        .ok_or_else(|| DispatchError::Protocol("authentication is required".to_owned()))?;

    let (mime_type, content) = match host {
        "registry" if uri.query().is_none() => {
            let origin = state.config.public_origin.as_deref().ok_or_else(|| {
                DispatchError::Tool("the registry has no configured public origin".to_owned())
            })?;
            let value = json!({
                "origin": origin,
                "api_url": format!("{origin}/api/v2"),
                "mcp_url": format!("{origin}/mcp"),
                "authorization_issuer": origin,
                "oauth_client_id": principal.oauth_client_id,
                "audience": principal.audience,
                "granted_scopes": principal.scopes,
            });
            ("application/json", pretty_json(&value)?)
        }
        "account" if uri.query().is_none() => {
            let value = json!({
                "id": user.id,
                "username": user.username,
                "name": user.name,
                "affiliation": user.affiliation,
                "is_member": user.is_member,
                "is_curator": user.is_curator,
                "is_admin": user.is_admin,
                "graph_uri": user.graph_uri,
            });
            ("application/json", pretty_json(&value)?)
        }
        "reviews" if uri.query().is_none() => {
            let items = state
                .app
                .review_service()
                .list_for(&user.graph_uri, user.is_admin)
                .await
                .map_err(tool_error)?;
            let value = json!({ "items": items, "total": items.len() });
            ("application/json", pretty_json(&value)?)
        }
        "design" => {
            let iri = query_parameter(&uri, "iri")?;
            let scope = state
                .app
                .acl_service
                .compute_scope(Some(&user.graph_uri))
                .await
                .map_err(tool_error)?;
            let details = state
                .app
                .object_details(&iri, scope)
                .await
                .map_err(tool_error)?
                .ok_or_else(|| {
                    DispatchError::Tool(
                        "design was not found or is not visible to this account".to_owned(),
                    )
                })?;
            let value = serde_json::to_value(details).map_err(tool_error)?;
            ("application/json", pretty_json(&value)?)
        }
        "design-content" => {
            let iri = query_parameter(&uri, "iri")?;
            let format =
                optional_query_parameter(&uri, "format").unwrap_or_else(|| "turtle".to_owned());
            let format = match format.as_str() {
                "turtle" => DownloadFormat::Turtle,
                "rdfxml" => DownloadFormat::Sbol,
                "jsonld" => DownloadFormat::JsonLd,
                "ntriples" => DownloadFormat::NTriples,
                other => {
                    return Err(DispatchError::Tool(format!(
                        "unsupported SBOL resource format `{other}`"
                    )))
                }
            };
            let scope = state
                .app
                .acl_service
                .compute_scope(Some(&user.graph_uri))
                .await
                .map_err(tool_error)?;
            let serialized = serialize_download(state, &iri, format, false, scope)
                .await
                .map_err(tool_error)?;
            if serialized.bytes.len() > MAX_EMBEDDED_DOWNLOAD_BYTES {
                return Err(DispatchError::Tool(format!(
                    "serialized design exceeds the MCP resource limit of {MAX_EMBEDDED_DOWNLOAD_BYTES} bytes"
                )));
            }
            let text = String::from_utf8(serialized.bytes).map_err(|_| {
                DispatchError::Tool("serialized SBOL resource was not UTF-8".to_owned())
            })?;
            (serialized.content_type, text)
        }
        _ => {
            return Err(DispatchError::Tool(format!(
                "unknown SBOL resource `{requested}`"
            )))
        }
    };
    Ok(json!({
        "contents": [{
            "uri": requested,
            "mimeType": mime_type,
            "text": content
        }]
    }))
}

fn query_parameter(uri: &Url, name: &str) -> Result<String, DispatchError> {
    optional_query_parameter(uri, name).ok_or_else(|| {
        DispatchError::Protocol(format!("resource URI requires a `{name}` query parameter"))
    })
}

fn optional_query_parameter(uri: &Url, name: &str) -> Option<String> {
    uri.query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
        .filter(|value| !value.trim().is_empty())
}

fn pretty_json(value: &Value) -> Result<String, DispatchError> {
    serde_json::to_string_pretty(value).map_err(tool_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_resource_requires_incremental_scope() {
        assert_eq!(
            required_scopes(&json!({ "uri": "sbol://reviews" })),
            &[SCOPE_READ, SCOPE_REVIEW]
        );
        assert_eq!(
            required_scopes(&json!({
                "uri": "sbol://design?iri=https%3A%2F%2Fsbol.io%2Fdesign%2F1"
            })),
            &[SCOPE_READ]
        );
    }
}
