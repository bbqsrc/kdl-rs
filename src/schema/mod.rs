//! KDL Schema validation support.
//!
//! This module provides the ability to validate KDL documents against
//! KDL Schema files as defined in the [KDL Schema Specification](https://github.com/kdl-org/kdl/blob/main/SCHEMA-SPEC.md).
//!
//! # Example
//!
//! ```rust
//! use kdl::{KdlDocument, schema::KdlSchema};
//!
//! let schema_src = r#"
//! document {
//!     node "person" {
//!         min 1
//!         prop "name" {
//!             required #true
//!             type "string"
//!         }
//!         value {
//!             min 0
//!             max 1
//!             type "number"
//!         }
//!     }
//! }
//! "#;
//!
//! let doc_src = r#"
//! person name="Alice" 30
//! "#;
//!
//! let schema = KdlSchema::parse(schema_src).expect("valid schema");
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

/// A KDL Schema used for validating KDL documents.
///
/// A schema is itself a KDL document with a specific structure defined by
/// the KDL Schema Specification.
#[derive(Debug, Clone)]
pub struct KdlSchema {
    doc: KdlDocument,
    input: Arc<String>,
}

impl KdlSchema {
    /// Creates a new schema from a parsed KDL document.
    ///
    /// The document must have a `document` node at the root level.
    pub fn new(doc: KdlDocument, input: impl Into<String>) -> Result<Self, KdlError> {
        let input = Arc::new(input.into());
        let schema = Self { doc, input };

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

    /// Parses a string into a KDL schema.
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

    /// Returns the `document` node from the schema.
    pub(crate) fn document_node(&self) -> Option<&KdlNode> {
        self.doc.get("document")
    }

    /// Returns the `definitions` node from the schema, if present.
    pub(crate) fn definitions(&self) -> Option<&KdlNode> {
        self.document_node()?.children()?.get("definitions")
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
        min 1
    }
}
"#;
        let schema = KdlSchema::parse(schema_src).unwrap();
        assert!(schema.document_node().is_some());
    }

    #[test]
    fn schema_requires_document_node() {
        let schema_src = "node \"test\"";
        let result = KdlSchema::parse(schema_src);
        assert!(result.is_err());
    }

    #[test]
    fn validate_simple_document() {
        let schema_src = r#"
document {
    node "person" {
        min 1
        max 10
    }
}
"#;
        let doc_src = r#"
person
person
"#;
        let schema = KdlSchema::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();

        let errors = schema.validate(&doc);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn validate_missing_required_node() {
        let schema_src = r#"
document {
    node "required-node" {
        min 1
    }
}
"#;
        let doc_src = "other-node\n";

        let schema = KdlSchema::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();

        let errors = schema.validate(&doc);
        assert!(
            !errors.is_empty(),
            "Expected errors for missing required node"
        );
    }

    #[test]
    fn validate_unknown_node_disallowed() {
        let schema_src = r#"
document {
    node "allowed" {
    }
}
"#;
        let doc_src = "unknown-node\n";

        let schema = KdlSchema::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();

        let errors = schema.validate(&doc);
        assert!(!errors.is_empty(), "Expected errors for unknown node");
    }

    #[test]
    fn validate_unknown_node_allowed() {
        let schema_src = r#"
document {
    node "known" {
    }
    other-nodes-allowed #true
}
"#;
        let doc_src = "unknown-node\n";

        let schema = KdlSchema::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();

        let errors = schema.validate(&doc);
        assert!(
            errors.is_empty(),
            "Expected no errors when other-nodes-allowed"
        );
    }

    // Property validation tests

    #[test]
    fn validate_required_property() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "person" {
        prop "name" {
            required #true
        }
    }
}
"#,
        )
        .unwrap();

        // Missing required prop - should error
        let doc: KdlDocument = "person".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());

        // Has required prop - should pass
        let doc: KdlDocument = "person name=\"Alice\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_other_props_allowed() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "person" {
        prop "name" { }
        other-props-allowed #true
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "person name=\"Alice\" age=30".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_unknown_prop_disallowed() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "person" {
        prop "name" { }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "person name=\"Alice\" age=30".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    // Type validation tests

    #[test]
    fn validate_string_type() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "item" {
        value {
            type "string"
        }
    }
}
"#,
        )
        .unwrap();

        // String value - pass
        let doc: KdlDocument = "item \"hello\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        // Number value - fail
        let doc: KdlDocument = "item 42".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_number_type() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "count" {
        value {
            type "number"
        }
    }
}
"#,
        )
        .unwrap();

        // Integer - pass
        let doc: KdlDocument = "count 42".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        // Float - pass
        let doc: KdlDocument = "count 3.14".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        // String - fail
        let doc: KdlDocument = "count \"hello\"".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_boolean_type() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "flag" {
        value {
            type "boolean"
        }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "flag #true".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "flag \"true\"".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    // Enum validation tests

    #[test]
    fn validate_enum() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "color" {
        value {
            enum "red" "green" "blue"
        }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "color \"red\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "color \"yellow\"".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    // Pattern validation tests

    #[test]
    fn validate_pattern() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "email" {
        value {
            pattern "^[a-z]+@[a-z]+\\.[a-z]+$"
        }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "email \"test@example.com\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "email \"invalid\"".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    // String length validation tests

    #[test]
    fn validate_min_max_length() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "code" {
        value {
            min-length 3
            max-length 10
        }
    }
}
"#,
        )
        .unwrap();

        // Too short
        let doc: KdlDocument = "code \"ab\"".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());

        // Just right
        let doc: KdlDocument = "code \"hello\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        // Too long
        let doc: KdlDocument = "code \"verylongstring\"".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    // Numeric constraint tests

    #[test]
    fn validate_numeric_range() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "age" {
        value {
            ">=" 0
            "<" 150
        }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "age 25".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "age -1".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());

        let doc: KdlDocument = "age 200".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_multiple_of() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "even" {
        value {
            "%" 2
        }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "even 4".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "even 3".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    // Integer format tests

    #[test]
    fn validate_integer_format() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "byte" {
        value {
            format "u8"
        }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "byte 255".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "byte 256".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());

        let doc: KdlDocument = "byte -1".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    // Value count tests

    #[test]
    fn validate_value_count() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "point" {
        value {
            min 2
            max 3
        }
    }
}
"#,
        )
        .unwrap();

        // Too few
        let doc: KdlDocument = "point 1".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());

        // Just right
        let doc: KdlDocument = "point 1 2".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        // Also okay
        let doc: KdlDocument = "point 1 2 3".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        // Too many
        let doc: KdlDocument = "point 1 2 3 4".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    // Children validation tests

    #[test]
    fn validate_children() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "parent" {
        children {
            node "child" {
                min 1
            }
        }
    }
}
"#,
        )
        .unwrap();

        // Has required child
        let doc: KdlDocument = "parent { child }".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        // Missing required child
        let doc: KdlDocument = "parent { }".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    // Ref resolution tests

    #[test]
    fn validate_ref_resolution() {
        let schema = KdlSchema::parse(
            r#"
document {
    definitions {
        value id="positive-int" {
            type "number"
            ">=" 0
        }
    }
    node "count" {
        value ref="positive-int"
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "count 10".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "count -5".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    // Tag validation tests

    #[test]
    fn validate_tag_allowed() {
        let schema = KdlSchema::parse(
            r#"
document {
    tag "important"
    other-tags-allowed #false
    other-nodes-allowed #true
}
"#,
        )
        .unwrap();

        // Allowed tag
        let doc: KdlDocument = "(important)item".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        // Unknown tag
        let doc: KdlDocument = "(unknown)item".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());

        // No tag - allowed
        let doc: KdlDocument = "item".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_other_tags_allowed() {
        let schema = KdlSchema::parse(
            r#"
document {
    tag "known"
    other-tags-allowed #true
    other-nodes-allowed #true
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "(unknown)item".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());
    }

    // Node-names validation tests

    #[test]
    fn validate_node_names_pattern() {
        let schema = KdlSchema::parse(
            r#"
document {
    node-names {
        pattern "^[a-z-]+$"
    }
    other-nodes-allowed #true
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "valid-name".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "Invalid_Name".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    // Prop-names validation tests

    #[test]
    fn validate_prop_names_pattern() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "item" {
        prop-names {
            pattern "^[a-z]+$"
        }
        other-props-allowed #true
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "item valid=\"x\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "item Invalid=\"x\"".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    // Tag-names validation tests

    #[test]
    fn validate_tag_names_pattern() {
        let schema = KdlSchema::parse(
            r#"
document {
    tag-names {
        pattern "^[A-Z][a-z]+$"
    }
    other-tags-allowed #true
    other-nodes-allowed #true
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "(Valid)item".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "(invalid)item".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    // Wildcard prop tests

    #[test]
    fn validate_wildcard_prop() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "data" {
        prop {
            type "string"
        }
        other-props-allowed #true
    }
}
"#,
        )
        .unwrap();

        // All props must be strings
        let doc: KdlDocument = "data foo=\"bar\" baz=\"qux\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "data foo=123".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    // String format tests

    #[test]
    fn validate_string_formats() {
        // Date-time
        let schema = KdlSchema::parse(
            r#"
document {
    node "timestamp" {
        value { format "date-time" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "timestamp \"2024-01-15T10:30:00\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "timestamp \"not-a-date\"".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());

        // UUID
        let schema = KdlSchema::parse(
            r#"
document {
    node "id" {
        value { format "uuid" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "id \"550e8400-e29b-41d4-a716-446655440000\""
            .parse()
            .unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "id \"not-a-uuid\"".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    // Float format tests

    #[test]
    fn validate_float_format() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "value" {
        value { format "f32" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "value 3.14".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());
    }

    // Value tag constraint tests

    #[test]
    fn validate_value_tag_constraint() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "typed" {
        value {
            tag {
                enum "i32" "u32" "f64"
            }
        }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "typed (i32)42".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "typed (unknown)42".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    // ========================================
    // Group 1: Core Node Features
    // ========================================

    #[test]
    fn validate_wildcard_node() {
        // Node definition without name applies to ALL nodes
        let schema = KdlSchema::parse(
            r#"
document {
    node {
        value {
            type "string"
        }
    }
    other-nodes-allowed #true
}
"#,
        )
        .unwrap();

        // All nodes must have string values
        let doc: KdlDocument = "foo \"hello\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "bar \"world\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        // Number value fails for any node
        let doc: KdlDocument = "baz 42".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_null_type() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "nullable" {
        value {
            type "null"
        }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "nullable #null".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "nullable \"not null\"".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());

        let doc: KdlDocument = "nullable 0".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_multiple_patterns() {
        // All patterns must match
        let schema = KdlSchema::parse(
            r#"
document {
    node "code" {
        value {
            pattern "^[A-Z]"
            pattern "[0-9]$"
        }
    }
}
"#,
        )
        .unwrap();

        // Starts with uppercase AND ends with digit
        let doc: KdlDocument = "code \"ABC123\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        // Only starts with uppercase (fails second pattern)
        let doc: KdlDocument = "code \"ABCDEF\"".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());

        // Only ends with digit (fails first pattern)
        let doc: KdlDocument = "code \"abc123\"".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_node_tag() {
        // Tag validation on node definition
        // Note: other-tags-allowed is needed because document-level tag whitelist
        // and node-level tag constraints are separate mechanisms per spec
        let schema = KdlSchema::parse(
            r#"
document {
    node "item" {
        tag {
            enum "important" "optional"
        }
    }
    other-tags-allowed #true
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "(important)item".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "(optional)item".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        // No tag is okay (tag not required)
        let doc: KdlDocument = "item".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        // Invalid tag - fails node-level tag constraint
        let doc: KdlDocument = "(invalid)item".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_multiple_values() {
        // Multiple value definitions for positional arguments
        let schema = KdlSchema::parse(
            r#"
document {
    node "point" {
        value {
            type "number"
        }
        value {
            type "number"
        }
    }
}
"#,
        )
        .unwrap();

        // Two number values
        let doc: KdlDocument = "point 10 20".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        // First value wrong type
        let doc: KdlDocument = "point \"x\" 20".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());

        // Second value wrong type
        let doc: KdlDocument = "point 10 \"y\"".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_children_node_names() {
        // node-names validation within children block
        let schema = KdlSchema::parse(
            r#"
document {
    node "parent" {
        children {
            node-names {
                pattern "^child-[a-z]+$"
            }
            other-nodes-allowed #true
        }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "parent { child-foo; child-bar }".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "parent { InvalidChild }".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    // ========================================
    // Group 2: Edge Cases
    // ========================================

    #[test]
    fn validate_min_zero_optional_node() {
        // min 0 makes node optional
        let schema = KdlSchema::parse(
            r#"
document {
    node "optional" {
        min 0
    }
    node "required" {
        min 1
    }
}
"#,
        )
        .unwrap();

        // Only required node present
        let doc: KdlDocument = "required".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        // Both present
        let doc: KdlDocument = "required\noptional".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_max_zero_disallowed_node() {
        // max 0 disallows node
        let schema = KdlSchema::parse(
            r#"
document {
    node "forbidden" {
        max 0
    }
    other-nodes-allowed #true
}
"#,
        )
        .unwrap();

        // No forbidden node
        let doc: KdlDocument = "other".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        // Forbidden node present
        let doc: KdlDocument = "forbidden".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_greater_than() {
        // > exclusive lower bound
        let schema = KdlSchema::parse(
            r#"
document {
    node "positive" {
        value {
            ">" 0
        }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "positive 1".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        // Exactly 0 should fail (exclusive)
        let doc: KdlDocument = "positive 0".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());

        let doc: KdlDocument = "positive -1".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_less_than_or_equal() {
        // <= inclusive upper bound
        let schema = KdlSchema::parse(
            r#"
document {
    node "percent" {
        value {
            "<=" 100
        }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "percent 100".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "percent 50".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "percent 101".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_enum_numbers() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "priority" {
        value {
            enum 1 2 3
        }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "priority 1".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "priority 3".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "priority 4".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_enum_booleans() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "flag" {
        value {
            enum #true #false
        }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "flag #true".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "flag #false".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "flag \"true\"".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_prop_tag() {
        // Tag validation on property values
        let schema = KdlSchema::parse(
            r#"
document {
    node "config" {
        prop "value" {
            tag {
                enum "i32" "f64"
            }
        }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "config value=(i32)42".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "config value=(f64)3.14".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "config value=(invalid)42".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    // ========================================
    // Group 3: Tag Features
    // ========================================

    #[test]
    fn validate_wildcard_tag() {
        // Tag without name applies to all tags
        let schema = KdlSchema::parse(
            r#"
document {
    tag {
        node-names {
            pattern "^[a-z]+$"
        }
        other-nodes-allowed #true
    }
    other-tags-allowed #true
    other-nodes-allowed #true
}
"#,
        )
        .unwrap();

        // Any tag with lowercase node name
        let doc: KdlDocument = "(custom)lowercase".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        // Tag with uppercase node name fails
        let doc: KdlDocument = "(custom)Uppercase".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_tag_node_constraints() {
        // Node definitions within tag
        let schema = KdlSchema::parse(
            r#"
document {
    tag "special" {
        node "allowed" { }
        other-nodes-allowed #false
    }
    other-tags-allowed #true
    other-nodes-allowed #true
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "(special)allowed".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "(special)forbidden".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());

        // Non-special tag, any node allowed
        let doc: KdlDocument = "(other)anything".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_tag_node_names() {
        // node-names within tag
        let schema = KdlSchema::parse(
            r#"
document {
    tag "typed" {
        node-names {
            pattern "^[A-Z][a-z]+$"
        }
        other-nodes-allowed #true
    }
    other-tags-allowed #true
    other-nodes-allowed #true
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "(typed)Hello".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "(typed)INVALID".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_tag_ref() {
        // ref on tag definitions
        let schema = KdlSchema::parse(
            r#"
document {
    definitions {
        tag id="strict-tag" {
            node "valid" { }
            other-nodes-allowed #false
        }
    }
    tag "myTag" ref="strict-tag"
    other-tags-allowed #false
    other-nodes-allowed #true
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "(myTag)valid".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "(myTag)invalid".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    // ========================================
    // Group 4: Ref/Definitions
    // ========================================

    #[test]
    fn validate_node_ref() {
        let schema = KdlSchema::parse(
            r#"
document {
    definitions {
        node id="string-node" {
            value {
                type "string"
            }
        }
    }
    node "item" ref="string-node"
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "item \"hello\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "item 42".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_prop_ref() {
        let schema = KdlSchema::parse(
            r#"
document {
    definitions {
        prop id="number-prop" {
            type "number"
        }
    }
    node "config" {
        prop "count" ref="number-prop"
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "config count=42".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "config count=\"text\"".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_children_ref() {
        let schema = KdlSchema::parse(
            r#"
document {
    definitions {
        children id="strict-children" {
            node "child" {
                min 1
            }
        }
    }
    node "parent" {
        children ref="strict-children"
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "parent { child }".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "parent { }".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_ref_merges_constraints() {
        // ref merges with local constraints
        let schema = KdlSchema::parse(
            r#"
document {
    definitions {
        value id="positive" {
            ">=" 0
        }
    }
    node "bounded" {
        value ref="positive" {
            "<=" 100
        }
    }
}
"#,
        )
        .unwrap();

        // Must satisfy both: >= 0 from ref AND <= 100 from local
        let doc: KdlDocument = "bounded 50".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "bounded -1".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());

        let doc: KdlDocument = "bounded 101".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    // ========================================
    // Group 5: String Formats
    // ========================================

    #[test]
    fn validate_format_time() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "time" {
        value { format "time" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "time \"10:30:00\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        // Note: time format validation uses regex, 25:00:00 may pass regex but isn't valid
        // Testing that the format is recognized and basic validation occurs
        let doc: KdlDocument = "time \"invalid-time\"".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_date() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "date" {
        value { format "date" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "date \"2024-01-15\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "date \"not-a-date\"".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_duration() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "duration" {
        value { format "duration" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "duration \"P1Y2M3DT4H5M6S\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "duration \"invalid\"".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_decimal() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "decimal" {
        value { format "decimal" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "decimal \"123.456\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "decimal \"-123.456\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_currency() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "currency" {
        value { format "currency" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "currency \"USD\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "currency \"EUR\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "currency \"INVALID\"".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_country_2() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "country" {
        value { format "country-2" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "country \"US\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "country \"GB\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        // Note: Implementation only checks format (2 uppercase letters), not actual ISO codes
        // "XX" passes because it's 2 uppercase letters
        let doc: KdlDocument = "country \"invalid\"".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty()); // lowercase fails
    }

    #[test]
    fn validate_format_country_3() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "country" {
        value { format "country-3" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "country \"USA\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        // Note: Implementation only checks format (3 uppercase letters), not actual ISO codes
        let doc: KdlDocument = "country \"invalid\"".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty()); // lowercase/wrong length fails
    }

    #[test]
    fn validate_format_country_subdivision() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "region" {
        value { format "country-subdivision" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "region \"US-CA\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        // Note: country-subdivision format returns true for all values (no validation implemented)
        // Just test that valid format is accepted
    }

    #[test]
    fn validate_format_email() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "email" {
        value { format "email" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "email \"user@example.com\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "email \"invalid-email\"".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_idn_email() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "email" {
        value { format "idn-email" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "email \"user@example.com\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        // Note: idn-email format returns true for all values (no validation implemented)
        // Just test that valid format is accepted
    }

    #[test]
    fn validate_format_hostname() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "host" {
        value { format "hostname" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "host \"example.com\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "host \"localhost\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "host \"invalid..hostname\"".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_idn_hostname() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "host" {
        value { format "idn-hostname" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "host \"example.com\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        // Note: idn-hostname format returns true for all values (no validation implemented)
        // Just test that valid format is accepted
    }

    #[test]
    fn validate_format_ipv4() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "ip" {
        value { format "ipv4" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "ip \"192.168.1.1\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "ip \"256.1.1.1\"".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_ipv6() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "ip" {
        value { format "ipv6" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "ip \"::1\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "ip \"2001:db8::1\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "ip \"invalid\"".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_url() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "url" {
        value { format "url" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "url \"https://example.com/path\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "url \"not a url\"".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_url_reference() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "ref" {
        value { format "url-reference" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "ref \"/path/to/resource\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "ref \"https://example.com\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_irl() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "irl" {
        value { format "irl" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "irl \"https://example.com\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_irl_reference() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "ref" {
        value { format "irl-reference" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "ref \"/path\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_url_template() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "template" {
        value { format "url-template" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "template \"https://example.com/{id}\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_regex() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "pattern" {
        value { format "regex" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "pattern \"^[a-z]+$\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        // Note: regex format returns true for all values (no validation implemented)
        // Just test that valid format is accepted
    }

    #[test]
    fn validate_format_base64() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "data" {
        value { format "base64" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "data \"SGVsbG8gV29ybGQ=\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "data \"not base64!\"".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_kdl_query() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "query" {
        value { format "kdl-query" }
    }
}
"#,
        )
        .unwrap();

        // KDL Query format - basic selector
        let doc: KdlDocument = "query \"node > child\"".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());
    }

    // ========================================
    // Group 6: Numeric Formats
    // ========================================

    #[test]
    fn validate_format_i8() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "byte" {
        value { format "i8" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "byte 127".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "byte -128".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "byte 128".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());

        let doc: KdlDocument = "byte -129".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_i16() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "short" {
        value { format "i16" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "short 32767".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "short -32768".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "short 32768".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_i32() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "int" {
        value { format "i32" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "int 2147483647".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "int -2147483648".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "int 2147483648".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_i64() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "long" {
        value { format "i64" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "long 9223372036854775807".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "long -9223372036854775808".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_i128() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "big" {
        value { format "i128" }
    }
}
"#,
        )
        .unwrap();

        // i128 max is very large, just test basic values
        let doc: KdlDocument = "big 0".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "big -1".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_u16() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "ushort" {
        value { format "u16" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "ushort 65535".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "ushort 0".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "ushort 65536".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());

        let doc: KdlDocument = "ushort -1".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_u32() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "uint" {
        value { format "u32" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "uint 4294967295".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "uint 4294967296".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_u64() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "ulong" {
        value { format "u64" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "ulong 18446744073709551615".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "ulong -1".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_u128() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "ubig" {
        value { format "u128" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "ubig 0".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "ubig -1".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_isize() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "size" {
        value { format "isize" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "size 0".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "size -1".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_usize() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "usize" {
        value { format "usize" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "usize 0".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "usize -1".parse().unwrap();
        assert!(!schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_f64() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "double" {
        value { format "f64" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "double 3.14159265358979".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());

        let doc: KdlDocument = "double 1.7976931348623157e308".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_decimal64() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "dec64" {
        value { format "decimal64" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "dec64 123.456".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());
    }

    #[test]
    fn validate_format_decimal128() {
        let schema = KdlSchema::parse(
            r#"
document {
    node "dec128" {
        value { format "decimal128" }
    }
}
"#,
        )
        .unwrap();

        let doc: KdlDocument = "dec128 123.456".parse().unwrap();
        assert!(schema.validate(&doc).is_empty());
    }
}
