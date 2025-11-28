//! KDL Schema v2 validation support.
//!
//! This module provides the ability to validate KDL documents against
//! KDL Schema v2 files as defined in the [KDL Schema Language Specification v2.0.0](https://github.com/kdl-org/kdl/blob/zkat/schema-v2/SCHEMA-SPEC.md).
//!
//! Note: This is an alpha implementation of the unreleased v2 schema spec.
//!
//! # Key Differences from Schema v1
//!
//! - `metadata` replaces `info` for schema metadata
//! - `arg`/`args` replace `value` for positional arguments
//! - `required`/`repeatable` replace `min`/`max` for cardinality
//! - `disallow-others` replaces `other-*-allowed` (inverted logic!)
//! - `props` aggregate node for property validations
//! - `gte`/`gt`/`lte`/`lt` replace `>=`/`>`/`<=`/`<`
//! - `@ksl:schema` directive for schema self-reference
//! - KPath-based `ref` system for referencing definitions
//!
//! # Example
//!
//! ```rust
//! use kdl::{KdlDocument, schema_v2::KdlSchemaV2};
//!
//! let schema_src = r#"
//! document {
//!     node "person" {
//!         required
//!         prop "name" {
//!             required
//!             type string
//!         }
//!         arg {
//!             optional
//!             type number
//!         }
//!     }
//! }
//! "#;
//!
//! let doc_src = r#"
//! person name="Alice" 30
//! "#;
//!
//! let schema = KdlSchemaV2::parse(schema_src).expect("valid schema");
//! let doc: KdlDocument = doc_src.parse().expect("valid document");
//!
//! let errors = schema.validate(&doc);
//! assert!(errors.is_empty());
//! ```

mod types;
mod validate;

pub use types::ValueFormat;

use std::sync::Arc;

#[cfg(feature = "span")]
use miette::SourceSpan;

use crate::{KdlDiagnostic, KdlDocument, KdlError, KdlNode};

/// A KDL Schema v2 used for validating KDL documents.
///
/// A schema is itself a KDL document with a specific structure defined by
/// the KDL Schema Language Specification v2.0.0.
#[derive(Debug, Clone)]
pub struct KdlSchemaV2 {
    doc: KdlDocument,
    input: Arc<String>,
    /// The schema URL from @ksl:schema directive, if present
    schema_url: Option<String>,
}

impl KdlSchemaV2 {
    /// Creates a new schema from a parsed KDL document.
    ///
    /// The document must have a `document` node at the root level.
    pub fn new(doc: KdlDocument, input: impl Into<String>) -> Result<Self, KdlError> {
        let input = Arc::new(input.into());

        // Check for @ksl:schema directive
        let schema_url = doc
            .get("@ksl:schema")
            .and_then(|n| n.get(0))
            .and_then(|v| v.as_string())
            .map(|s| s.to_string());

        let schema = Self {
            doc,
            input,
            schema_url,
        };

        // Validate that this is a valid schema structure
        if schema.document_node().is_none() {
            return Err(KdlError {
                input: schema.input.clone(),
                diagnostics: vec![KdlDiagnostic {
                    input: schema.input.clone(),
                    #[cfg(feature = "span")]
                    span: SourceSpan::new(0.into(), 0),
                    #[cfg(not(feature = "span"))]
                    span: miette::SourceSpan::new(0.into(), 0),
                    message: Some("Schema must have a 'document' node at root level".into()),
                    label: Some("missing 'document' node".into()),
                    help: Some("Add a 'document' node containing your schema definition".into()),
                    severity: miette::Severity::Error,
                }],
            });
        }

        Ok(schema)
    }

    /// Parses a string into a KDL schema v2.
    pub fn parse(input: &str) -> Result<Self, KdlError> {
        let doc: KdlDocument = input.parse()?;
        Self::new(doc, input)
    }

    /// Validates a KDL document against this schema.
    ///
    /// Returns a list of validation diagnostics. An empty list means
    /// the document is valid according to the schema.
    pub fn validate(&self, target: &KdlDocument) -> Vec<KdlDiagnostic> {
        validate::validate_document(self, target)
    }

    /// Returns a reference to the underlying KDL document.
    pub fn document(&self) -> &KdlDocument {
        &self.doc
    }

    /// Returns the schema URL from @ksl:schema directive, if present.
    pub fn schema_url(&self) -> Option<&str> {
        self.schema_url.as_deref()
    }

    /// Returns the `document` node from the schema.
    pub(crate) fn document_node(&self) -> Option<&KdlNode> {
        self.doc.get("document")
    }

    /// Returns the `metadata` node from the schema, if present.
    pub fn metadata(&self) -> Option<&KdlNode> {
        self.doc.get("metadata")
    }

    /// Returns the `definitions` node from the schema, if present.
    pub(crate) fn definitions(&self) -> Option<&KdlNode> {
        self.doc.get("definitions")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_schema() {
        let schema_src = r#"
document {
    node "test" {
        required
    }
}
"#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        assert!(schema.document_node().is_some());
    }

    #[test]
    fn schema_requires_document_node() {
        let schema_src = "node \"test\"";
        let result = KdlSchemaV2::parse(schema_src);
        assert!(result.is_err());
    }

    #[test]
    fn parse_schema_with_ksl_directive() {
        let schema_src = r#"
@ksl:schema "https://example.com/schema.kdl"

document {
    node "test"
}
"#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        assert_eq!(schema.schema_url(), Some("https://example.com/schema.kdl"));
    }

    #[test]
    fn parse_schema_with_metadata() {
        let schema_src = r#"
metadata {
    id "https://example.com/schema"
    title "Test Schema"
    description "A test schema"
}

document {
    node "test"
}
"#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        assert!(schema.metadata().is_some());
    }

    #[test]
    fn validate_simple_document() {
        let schema_src = r#"
document {
    node "person" {
        required
        repeatable max=10
    }
}
"#;
        let doc_src = "person";
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn validate_missing_required_node() {
        let schema_src = r#"
document {
    node "person" {
        required
    }
}
"#;
        let doc_src = "other";
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(!errors.is_empty());
    }

    #[test]
    fn validate_disallow_others() {
        let schema_src = r#"
document {
    children {
        disallow-others
        node "allowed"
    }
}
"#;
        let doc_src = "notallowed";
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(!errors.is_empty());
    }

    #[test]
    fn validate_arg_required_by_default() {
        let schema_src = r#"
document {
    node "test" {
        arg {
            type string
        }
    }
}
"#;
        let doc_src = "test"; // missing required arg
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(!errors.is_empty());
    }

    #[test]
    fn validate_arg_optional() {
        let schema_src = r#"
document {
    node "test" {
        arg {
            optional
            type string
        }
    }
}
"#;
        let doc_src = "test"; // missing optional arg is OK
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn validate_prop_with_type() {
        let schema_src = r#"
document {
    node "test" {
        prop "name" {
            type string
        }
    }
}
"#;
        let doc_src = r#"test name="hello""#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn validate_prop_wrong_type() {
        let schema_src = r#"
document {
    node "test" {
        prop "name" {
            type string
        }
    }
}
"#;
        let doc_src = "test name=42"; // number instead of string
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(!errors.is_empty());
    }

    #[test]
    fn validate_repeatable_node() {
        let schema_src = r#"
document {
    node "item" {
        repeatable
    }
}
"#;
        let doc_src = r#"
item
item
item
"#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn validate_non_repeatable_node() {
        let schema_src = r#"
document {
    node "item"
}
"#;
        let doc_src = r#"
item
item
"#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(!errors.is_empty());
    }

    #[test]
    fn validate_numeric_gte() {
        let schema_src = r#"
document {
    node "test" {
        arg {
            type number
            gte 0
        }
    }
}
"#;
        let doc_src = "test 5";
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn validate_numeric_gte_fail() {
        let schema_src = r#"
document {
    node "test" {
        arg {
            type number
            gte 10
        }
    }
}
"#;
        let doc_src = "test 5";
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(!errors.is_empty());
    }

    #[test]
    fn validate_format_multiple() {
        let schema_src = r#"
document {
    node "test" {
        arg {
            type string
            format url irl
        }
    }
}
"#;
        let doc_src = r#"test "https://example.com""#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn validate_deprecated_node() {
        let schema_src = r#"
document {
    node "old-api" {
        deprecated message="Use new-api instead" by="new-api"
    }
}
"#;
        let doc_src = "old-api";
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        // Should have a warning for deprecated node
        assert!(!errors.is_empty());
        assert!(
            errors[0].message.as_ref().unwrap().contains("deprecated")
                || errors[0].message.as_ref().unwrap().contains("Use new-api")
        );
    }

    #[test]
    fn validate_distinct_args() {
        let schema_src = r#"
document {
    node "tags" {
        args {
            type string
            distinct
        }
    }
}
"#;
        let doc_src = r#"tags "a" "b" "a""#; // "a" appears twice
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(!errors.is_empty(), "Expected error for duplicate args");
        assert!(errors[0].message.as_ref().unwrap().contains("Duplicate"));
    }

    #[test]
    fn validate_distinct_args_valid() {
        let schema_src = r#"
document {
    node "tags" {
        args {
            type string
            distinct
        }
    }
}
"#;
        let doc_src = r#"tags "a" "b" "c""#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn validate_disallow_node() {
        let schema_src = r#"
document {
    children {
        node "allowed"
        disallow {
            node "forbidden" {
                message "This node is forbidden!"
            }
        }
    }
}
"#;
        let doc_src = "forbidden";
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(!errors.is_empty());
        assert!(errors[0].message.as_ref().unwrap().contains("forbidden"));
    }

    #[test]
    fn validate_one_of_success() {
        let schema_src = r#"
document {
    children {
        one-of {
            choice {
                node "string-value" {
                    required
                    arg { type string }
                }
            }
            choice {
                node "number-value" {
                    required
                    arg { type number }
                }
            }
        }
    }
}
"#;
        let doc_src = "number-value 42";
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn validate_one_of_fail() {
        let schema_src = r#"
document {
    children {
        one-of {
            choice {
                node "option-a" { required }
            }
            choice {
                node "option-b" { required }
            }
        }
    }
}
"#;
        let doc_src = "option-c"; // Neither option-a nor option-b
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(!errors.is_empty());
        assert!(errors[0].message.as_ref().unwrap().contains("alternative"));
    }

    #[test]
    fn validate_nested_children() {
        let schema_src = r#"
document {
    node "parent" {
        children {
            node "child" {
                required
                arg { type string }
            }
        }
    }
}
"#;
        let doc_src = r#"
parent {
    child "hello"
}
"#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn validate_nested_children_missing() {
        let schema_src = r#"
document {
    node "parent" {
        children {
            node "child" {
                required
            }
        }
    }
}
"#;
        let doc_src = r#"
parent {
    other
}
"#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(
            !errors.is_empty(),
            "Expected error for missing required child"
        );
    }

    #[test]
    fn validate_repeatable_min_max() {
        let schema_src = r#"
document {
    node "item" {
        repeatable min=2 max=4
    }
}
"#;
        let doc_src = r#"
item
item
item
"#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn validate_repeatable_min_fail() {
        let schema_src = r#"
document {
    node "item" {
        repeatable min=2
    }
}
"#;
        let doc_src = "item"; // Only 1, need at least 2
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(!errors.is_empty());
        assert!(errors[0].message.as_ref().unwrap().contains("Too few"));
    }

    #[test]
    fn validate_repeatable_max_fail() {
        let schema_src = r#"
document {
    node "item" {
        repeatable max=2
    }
}
"#;
        let doc_src = r#"
item
item
item
"#; // 3 items, max is 2
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(!errors.is_empty());
        assert!(errors[0].message.as_ref().unwrap().contains("Too many"));
    }

    #[test]
    fn validate_required_prop() {
        let schema_src = r#"
document {
    node "user" {
        prop "name" {
            required
            type string
        }
    }
}
"#;
        let doc_src = "user"; // missing required prop "name"
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(!errors.is_empty());
        assert!(errors[0].message.as_ref().unwrap().contains("name"));
    }

    #[test]
    fn validate_children_min_max() {
        let schema_src = r#"
document {
    node "container" {
        children {
            min 2
            max 4
            node "item" { repeatable }
        }
    }
}
"#;
        let doc_src = r#"
container {
    item
    item
    item
}
"#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn validate_children_too_few() {
        let schema_src = r#"
document {
    node "container" {
        children {
            min 3
            node "item"
        }
    }
}
"#;
        let doc_src = r#"
container {
    item
}
"#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(!errors.is_empty());
        assert!(errors[0].message.as_ref().unwrap().contains("Too few"));
    }

    #[test]
    fn validate_enum_constraint() {
        let schema_src = r#"
document {
    node "level" {
        arg {
            type string
            enum "low" "medium" "high"
        }
    }
}
"#;
        let doc_src = r#"level "medium""#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn validate_enum_constraint_fail() {
        let schema_src = r#"
document {
    node "level" {
        arg {
            type string
            enum "low" "medium" "high"
        }
    }
}
"#;
        let doc_src = r#"level "ultra""#; // Not in enum
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(!errors.is_empty());
    }

    #[test]
    fn validate_pattern_constraint() {
        let schema_src = r#"
document {
    node "id" {
        arg {
            type string
            pattern "^[A-Z]{3}-\\d{4}$"
        }
    }
}
"#;
        let doc_src = r#"id "ABC-1234""#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn validate_pattern_constraint_fail() {
        let schema_src = r#"
document {
    node "id" {
        arg {
            type string
            pattern "^[A-Z]{3}-\\d{4}$"
        }
    }
}
"#;
        let doc_src = r#"id "invalid""#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(!errors.is_empty());
    }

    #[test]
    fn validate_numeric_range() {
        let schema_src = r#"
document {
    node "percent" {
        arg {
            type number
            gte 0
            lte 100
        }
    }
}
"#;
        let doc_src = "percent 50";
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn validate_numeric_lt_gt() {
        let schema_src = r#"
document {
    node "score" {
        arg {
            type number
            gt 0
            lt 10
        }
    }
}
"#;
        let doc_src = "score 5";
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn validate_props_required_list() {
        let schema_src = r#"
document {
    node "config" {
        props {
            required "host" "port"
        }
        prop "host" { type string }
        prop "port" { type number }
    }
}
"#;
        let doc_src = r#"config host="localhost" port=8080"#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn validate_props_required_list_fail() {
        let schema_src = r#"
document {
    node "config" {
        props {
            required "host" "port"
        }
        prop "host" { type string }
        prop "port" { type number }
    }
}
"#;
        let doc_src = r#"config host="localhost""#; // missing "port"
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(!errors.is_empty());
        assert!(errors[0].message.as_ref().unwrap().contains("port"));
    }

    // Tests for new features: ref with KQL, annotations, default, undefine

    #[test]
    fn test_ref_with_id_lookup() {
        let schema_src = r#"
definitions {
    node "base-type" id="string-arg" {
        arg { type string }
    }
}
document {
    node "derived" {
        ref "string-arg"
    }
}
"#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();

        // Valid - has string arg as defined in referenced definition
        let doc_valid: KdlDocument = r#"derived "hello""#.parse().unwrap();
        let errors = schema.validate(&doc_valid);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);

        // Invalid - has number arg instead of string
        let doc_invalid: KdlDocument = "derived 42".parse().unwrap();
        let errors = schema.validate(&doc_invalid);
        assert!(!errors.is_empty(), "Expected type error for number arg");
    }

    #[test]
    fn test_ref_with_kql_path() {
        let schema_src = r#"
definitions {
    node "base-type" id="base" {
        arg { type string }
    }
}
document {
    node "derived" {
        ref "node[id=\"base\"]"
    }
}
"#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();

        // Valid - has string arg as required by ref
        let doc_valid: KdlDocument = r#"derived "hello""#.parse().unwrap();
        let errors = schema.validate(&doc_valid);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);

        // Invalid - missing required arg (inherited from ref)
        let doc_invalid: KdlDocument = "derived".parse().unwrap();
        let errors = schema.validate(&doc_invalid);
        assert!(
            !errors.is_empty(),
            "Expected error for missing required arg"
        );
    }

    #[test]
    fn test_undefine_node() {
        let schema_src = r#"
document {
    children {
        node "allowed"
        undefine {
            node "forbidden"
        }
        disallow-others
    }
}
"#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();

        // Valid - "allowed" is defined
        let doc_valid: KdlDocument = "allowed".parse().unwrap();
        let errors = schema.validate(&doc_valid);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);

        // Invalid - "forbidden" is explicitly undefined
        let doc_invalid: KdlDocument = "forbidden".parse().unwrap();
        let errors = schema.validate(&doc_invalid);
        assert!(
            !errors.is_empty(),
            "Expected error for undefined node 'forbidden'"
        );
    }

    #[test]
    fn test_undefine_removes_from_validation() {
        // This tests that undefine actually removes a node definition
        // even if the node was previously defined
        let schema_src = r#"
document {
    children {
        node "keep-this"
        node "remove-this"
        undefine {
            node "remove-this"
        }
        disallow-others
    }
}
"#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();

        // "remove-this" should be rejected because it was undefined
        let doc: KdlDocument = "remove-this".parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(
            !errors.is_empty(),
            "Expected error - 'remove-this' was undefined"
        );

        // "keep-this" should work fine
        let doc: KdlDocument = "keep-this".parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn test_annotations_enum() {
        let schema_src = r#"
document {
    node "value" {
        annotations {
            enum "string" "number" "bool"
        }
    }
}
"#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();

        // Valid - type annotation is in enum
        let doc_valid: KdlDocument = "(string)value".parse().unwrap();
        let errors = schema.validate(&doc_valid);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);

        // Invalid - type annotation not in enum
        let doc_invalid: KdlDocument = "(unknown)value".parse().unwrap();
        let errors = schema.validate(&doc_invalid);
        assert!(!errors.is_empty(), "Expected error for invalid annotation");
    }

    #[test]
    fn test_annotations_pattern() {
        let schema_src = r#"
document {
    node "typed" {
        annotations {
            pattern "^(i|u)(8|16|32|64)$"
        }
    }
}
"#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();

        // Valid - matches pattern
        let doc_valid: KdlDocument = "(i32)typed".parse().unwrap();
        let errors = schema.validate(&doc_valid);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);

        // Invalid - doesn't match pattern
        let doc_invalid: KdlDocument = "(float)typed".parse().unwrap();
        let errors = schema.validate(&doc_invalid);
        assert!(
            !errors.is_empty(),
            "Expected error for annotation not matching pattern"
        );
    }

    #[test]
    fn test_annotations_missing_error() {
        let schema_src = r#"
document {
    node "must-have-type" {
        annotations {
            enum "a" "b"
        }
    }
}
"#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();

        // Invalid - missing required type annotation
        let doc: KdlDocument = "must-have-type".parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(
            !errors.is_empty(),
            "Expected error for missing type annotation"
        );
        assert!(
            errors[0]
                .message
                .as_ref()
                .unwrap()
                .contains("type annotation"),
            "Error should mention type annotation"
        );
    }

    #[test]
    fn test_default_arg_no_error() {
        let schema_src = r#"
document {
    node "config" {
        arg {
            type number
            default 42
        }
    }
}
"#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();

        // Valid - missing arg but has default, so no error
        let doc: KdlDocument = "config".parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(
            errors.is_empty(),
            "Expected no error - arg has default. Got: {:?}",
            errors
        );

        // Also valid - providing the arg explicitly
        let doc: KdlDocument = "config 100".parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn test_default_prop_no_error() {
        let schema_src = r#"
document {
    node "settings" {
        prop "timeout" {
            required
            type number
            default 30
        }
    }
}
"#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();

        // Valid - missing required prop but has default, so no error
        let doc: KdlDocument = "settings".parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(
            errors.is_empty(),
            "Expected no error - prop has default. Got: {:?}",
            errors
        );

        // Also valid - providing the prop explicitly
        let doc: KdlDocument = "settings timeout=60".parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn test_default_without_default_still_errors() {
        // Sanity check: args without default should still error when missing
        let schema_src = r#"
document {
    node "required-arg" {
        arg {
            type number
        }
    }
}
"#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();

        let doc: KdlDocument = "required-arg".parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(
            !errors.is_empty(),
            "Expected error - arg is required and has no default"
        );
    }
}
