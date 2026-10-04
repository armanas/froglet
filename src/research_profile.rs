//! Optional publisher declarations carried in an existing service schema.
//! These checks establish structural compatibility, never scientific truth.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const ANNOTATION: &str = "x-froglet-research-profile";
const VERSION: &str = "froglet.research-profile/v1";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchProfile {
    pub schema_version: String,
    pub collections: BTreeMap<String, Collection>,
    pub provenance: Provenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mapping_assumptions: Option<MappingAssumptions>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Collection {
    pub fields: BTreeMap<String, Field>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    #[serde(rename = "type")]
    pub kind: String,
    pub nullable: bool,
    /// Exact vocabulary token; "none" explicitly means no physical unit.
    pub unit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<Identifier>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Identifier {
    pub namespace: String,
    pub version: String,
    pub prefix: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub source: String,
    pub version: String,
    pub citation: String,
    pub license: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MappingAssumptions {
    pub collection: String,
    pub source_field: String,
    pub target_field: String,
    pub comparison: String,
    pub cardinality: String,
    pub duplicate_policy: String,
}

fn text(value: &str, path: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > 1024 || value.chars().any(char::is_control) {
        return Err(format!(
            "research_profile.{path} must contain 1–1024 non-control bytes"
        ));
    }
    Ok(())
}

impl ResearchProfile {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != VERSION {
            return Err("unsupported research_profile.schema_version".into());
        }
        let bytes = serde_json::to_vec(self).map_err(|error| error.to_string())?;
        if bytes.len() > 32 * 1024 || self.collections.is_empty() || self.collections.len() > 32 {
            return Err("research_profile requires 1–32 collections and at most 32 KiB".into());
        }
        for (name, collection) in &self.collections {
            text(name, "collection name")?;
            if collection.fields.is_empty() || collection.fields.len() > 256 {
                return Err(format!(
                    "research_profile.collections.{name} requires 1–256 fields"
                ));
            }
            for (name, field) in &collection.fields {
                text(name, "field name")?;
                text(&field.unit, "field.unit")?;
                if !matches!(
                    field.kind.as_str(),
                    "string" | "integer" | "number" | "boolean"
                ) {
                    return Err(format!(
                        "unsupported research_profile field type {:?}",
                        field.kind
                    ));
                }
                if let Some(identifier) = &field.identifier {
                    if field.kind != "string" {
                        return Err("research_profile identifiers require string fields".into());
                    }
                    text(&identifier.namespace, "identifier.namespace")?;
                    text(&identifier.version, "identifier.version")?;
                    text(&identifier.prefix, "identifier.prefix")?;
                }
            }
        }
        for (name, value) in [
            ("source", &self.provenance.source),
            ("version", &self.provenance.version),
            ("citation", &self.provenance.citation),
            ("license", &self.provenance.license),
        ] {
            text(value, &format!("provenance.{name}"))?;
        }
        if let Some(mapping) = &self.mapping_assumptions {
            let collection = self
                .collections
                .get(&mapping.collection)
                .ok_or("research_profile mapping collection is absent")?;
            if mapping.source_field == mapping.target_field {
                return Err("research_profile mapping source and target must differ".into());
            }
            for name in [&mapping.source_field, &mapping.target_field] {
                let field = collection
                    .fields
                    .get(name)
                    .ok_or("research_profile mapping field is absent")?;
                if field.kind != "string" || field.identifier.is_none() {
                    return Err(
                        "research_profile mapping fields require declared string identifiers"
                            .into(),
                    );
                }
            }
            text(&mapping.comparison, "mapping_assumptions.comparison")?;
            text(&mapping.cardinality, "mapping_assumptions.cardinality")?;
            text(
                &mapping.duplicate_policy,
                "mapping_assumptions.duplicate_policy",
            )?;
        }
        Ok(())
    }

    pub fn validate_rows(
        &self,
        data: &BTreeMap<String, Vec<Map<String, Value>>>,
    ) -> Result<(), String> {
        self.validate()?;
        if self.collections.keys().collect::<Vec<_>>() != data.keys().collect::<Vec<_>>() {
            return Err(
                "research_profile collections must exactly match the selected snapshot".into(),
            );
        }
        for (name, rows) in data {
            if rows.is_empty() {
                return Err(format!(
                    "research_profile collection {name} must have selected rows to validate its fields"
                ));
            }
            let fields = &self.collections[name].fields;
            let declared: BTreeSet<_> = fields.keys().collect();
            for (index, row) in rows.iter().enumerate() {
                if row.keys().collect::<BTreeSet<_>>() != declared {
                    return Err(format!(
                        "research_profile row fields differ at {name}[{index}]"
                    ));
                }
                for (name, value) in row {
                    let field = &fields[name];
                    if value.is_null() && field.nullable {
                        continue;
                    }
                    let valid = match field.kind.as_str() {
                        "string" => value.is_string(),
                        "integer" => value.is_i64() || value.is_u64(),
                        "number" => value.is_number(),
                        "boolean" => value.is_boolean(),
                        _ => false,
                    };
                    if !valid {
                        return Err(format!(
                            "research_profile type mismatch at row {index}, field {name}"
                        ));
                    }
                    if let Some(identifier) = &field.identifier
                        && !value.as_str().unwrap().starts_with(&identifier.prefix)
                    {
                        return Err(format!(
                            "research_profile identifier prefix mismatch at row {index}, field {name}"
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

/// Preserve only the known annotation, after validating the actual JSON bytes.
/// All structural schema keywords must still match the generated data schema.
pub fn annotated_output_schema(
    generated: &Value,
    authored: Option<&Value>,
    snapshot: &[u8],
) -> Result<Value, String> {
    let mut output = generated.clone();
    let Some(profile) = authored.and_then(|schema| schema.get(ANNOTATION)) else {
        return Ok(output);
    };
    let mut structural = authored.unwrap().clone();
    structural
        .as_object_mut()
        .ok_or("research profile output schema must be an object")?
        .remove(ANNOTATION);
    if structural != *generated {
        return Err("research profile must annotate the exact generated data output schema".into());
    }
    let raw_profile = profile;
    let profile: ResearchProfile = serde_json::from_value(raw_profile.clone())
        .map_err(|error| format!("invalid research_profile: {error}"))?;
    let canonical_profile = serde_json::to_value(&profile).map_err(|error| error.to_string())?;
    if canonical_profile != *raw_profile {
        return Err("research_profile must use object fields and omit absent optional declarations instead of null".into());
    }
    let data = serde_json::from_slice(snapshot)
        .map_err(|_| "research_profile requires a selected JSON snapshot")?;
    profile.validate_rows(&data)?;
    output[ANNOTATION] = canonical_profile;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> (ResearchProfile, BTreeMap<String, Vec<Map<String, Value>>>) {
        let profile = serde_json::from_value(json!({
            "schema_version":VERSION,"collections":{"rows":{"fields":{
                "id":{"type":"string","nullable":false,"unit":"none","identifier":{"namespace":"urn:demo","version":"1","prefix":"DEMO:"}},
                "value":{"type":"number","nullable":true,"unit":"mg/L"}}}},
            "provenance":{"source":"urn:synthetic","version":"1","citation":"Synthetic test fixture","license":"CC0-1.0"}
        })).unwrap();
        let data = serde_json::from_value(
            json!({"rows":[{"id":"DEMO:a","value":2.5},{"id":"DEMO:b","value":null}]}),
        )
        .unwrap();
        (profile, data)
    }

    #[test]
    fn validates_every_row_type_prefix_and_selected_field() {
        let (profile, mut data) = fixture();
        profile.validate_rows(&data).unwrap();
        data.get_mut("rows").unwrap()[1].insert("id".into(), json!("OTHER:b"));
        assert!(
            profile
                .validate_rows(&data)
                .unwrap_err()
                .contains("prefix mismatch")
        );
        data.get_mut("rows").unwrap()[1].insert("id".into(), json!("DEMO:b"));
        data.get_mut("rows").unwrap()[1].insert("value".into(), json!("2.5"));
        assert!(
            profile
                .validate_rows(&data)
                .unwrap_err()
                .contains("type mismatch")
        );
        data.get_mut("rows").unwrap()[1].insert("private".into(), json!("secret"));
        assert!(
            profile
                .validate_rows(&data)
                .unwrap_err()
                .contains("row fields differ")
        );
    }

    #[test]
    fn empty_collection_cannot_claim_unobserved_selected_fields() {
        let (profile, mut data) = fixture();
        data.get_mut("rows").unwrap().clear();
        assert!(
            profile
                .validate_rows(&data)
                .unwrap_err()
                .contains("must have selected rows")
        );
    }

    #[test]
    fn rejects_unknown_fields_versions_and_nonstrings_identifiers() {
        let (mut profile, _) = fixture();
        profile.schema_version = "future".into();
        assert!(profile.validate().is_err());
        profile.schema_version = VERSION.into();
        profile
            .collections
            .get_mut("rows")
            .unwrap()
            .fields
            .get_mut("id")
            .unwrap()
            .kind = "integer".into();
        assert!(
            profile
                .validate()
                .unwrap_err()
                .contains("identifiers require string")
        );
        let (profile, _) = fixture();
        let mut value = serde_json::to_value(profile).unwrap();
        value["unrecognized"] = json!(true);
        assert!(serde_json::from_value::<ResearchProfile>(value).is_err());
    }

    #[test]
    fn preserves_only_validated_annotation_and_refuses_structural_override() {
        let (profile, data) = fixture();
        let generated = crate::builtins::data_query::data_query_output_schema(&BTreeMap::from([(
            "rows".into(),
            vec!["id".into(), "value".into()],
        )]));
        let mut authored = generated.clone();
        authored[ANNOTATION] = serde_json::to_value(profile).unwrap();
        let bytes = serde_json::to_vec(&data).unwrap();
        assert_eq!(
            annotated_output_schema(&generated, Some(&authored), &bytes).unwrap(),
            authored
        );
        authored["type"] = json!("array");
        assert!(annotated_output_schema(&generated, Some(&authored), &bytes).is_err());
        assert_eq!(
            annotated_output_schema(&generated, None, &bytes).unwrap(),
            generated
        );
    }

    #[test]
    fn refuses_null_optional_and_positional_profile_forms_before_signing() {
        let (profile, data) = fixture();
        let generated = crate::builtins::data_query::data_query_output_schema(&BTreeMap::from([(
            "rows".into(),
            vec!["id".into(), "value".into()],
        )]));
        let canonical_profile = serde_json::to_value(profile).unwrap();
        let bytes = serde_json::to_vec(&data).unwrap();
        for raw in [
            {
                let mut value = canonical_profile.clone();
                value["mapping_assumptions"] = Value::Null;
                value
            },
            {
                let mut value = canonical_profile.clone();
                value["collections"]["rows"]["fields"]["value"]["identifier"] = Value::Null;
                value
            },
            json!([
                canonical_profile["schema_version"],
                canonical_profile["collections"],
                canonical_profile["provenance"]
            ]),
        ] {
            let mut authored = generated.clone();
            authored[ANNOTATION] = raw;
            assert!(annotated_output_schema(&generated, Some(&authored), &bytes).is_err());
        }
    }
}
