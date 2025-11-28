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

use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(feature = "span")]
use miette::SourceSpan;

use crate::{KdlDiagnostic, KdlDocument, KdlError, KdlNode};

/// A schema directive extracted from a document's `@ksl:schema` node.
///
/// Documents can reference one or more schemas using the `@ksl:schema` directive.
/// Per spec, ALL referenced schemas must validate for the document to pass.
#[derive(Debug, Clone)]
pub struct SchemaDirective {
    /// Path to the schema file (may be relative or absolute)
    pub path: String,
    /// Span of the directive in the source document
    #[cfg(feature = "span")]
    pub span: SourceSpan,
    /// Whether validation failures should produce warnings instead of errors
    pub warn_only: bool,
}

/// How schemas were specified for a document.
#[derive(Debug, Clone)]
pub enum SchemaSource {
    /// From `@ksl:schema` directive(s) in the document.
    /// Per spec: ALL schemas must validate for the document to pass.
    Directives(Vec<SchemaDirective>),
    /// No schema directive found in the document.
    None,
}

/// Result of validating a document against its `@ksl:schema` directives.
#[derive(Debug, Clone, Default)]
pub struct ValidationResult {
    /// All diagnostics from all schemas
    pub diagnostics: Vec<KdlDiagnostic>,
    /// Schema paths that were successfully loaded and validated
    pub validated_schemas: Vec<PathBuf>,
    /// Schema paths that failed to load (resolved path, original path string)
    pub failed_schemas: Vec<(PathBuf, String)>,
}

/// Resolve a schema path relative to a document's directory.
///
/// If the path is absolute, it is returned unchanged.
/// If relative, it is resolved against `document_dir` and canonicalized.
pub fn resolve_schema_path(schema_path: &str, document_dir: &Path) -> PathBuf {
    let path = Path::new(schema_path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        let resolved = document_dir.join(path);
        resolved.canonicalize().unwrap_or(resolved)
    }
}

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

    /// Load and parse a schema from a file path.
    ///
    /// # Errors
    /// Returns an error if the file cannot be read or if parsing fails.
    pub fn load_from_path(path: &Path) -> Result<Self, KdlError> {
        let content = std::fs::read_to_string(path).map_err(|e| KdlError {
            input: Arc::new(String::new()),
            diagnostics: vec![KdlDiagnostic {
                input: Arc::new(String::new()),
                #[cfg(feature = "span")]
                span: SourceSpan::new(0.into(), 0),
                #[cfg(not(feature = "span"))]
                span: miette::SourceSpan::new(0.into(), 0),
                message: Some(format!(
                    "Failed to read schema file '{}': {}",
                    path.display(),
                    e
                )),
                label: None,
                help: None,
                severity: miette::Severity::Error,
            }],
        })?;
        Self::parse(&content)
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

    /// Returns the schema's identifier from `metadata > id`, if present.
    ///
    /// This is typically a URL that serves as the base for resolving relative
    /// schema references in `ref` nodes.
    pub fn metadata_id(&self) -> Option<&str> {
        let metadata = self.metadata()?;
        let children = metadata.children()?;
        let id_node = children.get("id")?;
        id_node.get(0)?.as_string()
    }

    /// Returns the `definitions` node from the schema, if present.
    pub(crate) fn definitions(&self) -> Option<&KdlNode> {
        self.doc.get("definitions")
    }

    /// Extract `@ksl:schema` directives from a KDL document.
    ///
    /// This finds all `@ksl:schema` nodes in the document and extracts their
    /// schema paths and options. Per spec, multiple schemas can be specified
    /// and ALL must validate for the document to pass.
    ///
    /// # Example
    ///
    /// ```kdl
    /// @ksl:schema "schema.kdl"
    /// @ksl:schema "extra.kdl" warn-only=#true
    /// ```
    pub fn extract_directives(doc: &KdlDocument) -> SchemaSource {
        let mut directives = Vec::new();

        for node in doc.nodes() {
            if node.name().value() == "@ksl:schema" {
                #[cfg(feature = "span")]
                let span = node.span();

                let warn_only = node
                    .entry("warn-only")
                    .and_then(|e| e.value().as_bool())
                    .unwrap_or(false);

                // Each positional argument is a schema path
                for entry in node.entries() {
                    if entry.name().is_none() {
                        if let Some(path) = entry.value().as_string() {
                            directives.push(SchemaDirective {
                                path: path.to_string(),
                                #[cfg(feature = "span")]
                                span,
                                warn_only,
                            });
                        }
                    }
                }
            }
        }

        if directives.is_empty() {
            SchemaSource::None
        } else {
            SchemaSource::Directives(directives)
        }
    }

    /// Validates a KDL document against this schema, with severity control.
    ///
    /// If `warn_only` is true, all errors are downgraded to warnings.
    /// This is used when the `@ksl:schema` directive has `warn-only=#true`.
    pub fn validate_with_severity(
        &self,
        target: &KdlDocument,
        warn_only: bool,
    ) -> Vec<KdlDiagnostic> {
        let mut diags = self.validate(target);
        if warn_only {
            for diag in &mut diags {
                if diag.severity == miette::Severity::Error {
                    diag.severity = miette::Severity::Warning;
                }
            }
        }
        diags
    }
}

/// Validate a document against all its `@ksl:schema` directives.
///
/// This extracts schema directives from the document, loads each schema,
/// and validates the document against all of them. Per spec, ALL schemas
/// must pass for the document to be valid.
///
/// # Arguments
/// * `doc` - The document to validate
/// * `document_dir` - Directory containing the document (for resolving relative paths)
///
/// # Returns
/// A [`ValidationResult`] containing all diagnostics and status of each schema.
pub fn validate_with_directives(doc: &KdlDocument, document_dir: &Path) -> ValidationResult {
    let mut result = ValidationResult::default();

    let source = KdlSchemaV2::extract_directives(doc);

    match source {
        SchemaSource::Directives(directives) => {
            for directive in directives {
                let schema_path = resolve_schema_path(&directive.path, document_dir);

                match KdlSchemaV2::load_from_path(&schema_path) {
                    Ok(schema) => {
                        let diags = schema.validate_with_severity(doc, directive.warn_only);
                        result.diagnostics.extend(diags);
                        result.validated_schemas.push(schema_path);
                    }
                    Err(e) => {
                        let msg = format!(
                            "Failed to load schema '{}': {}",
                            directive.path,
                            e.diagnostics
                                .first()
                                .and_then(|d| d.message.as_ref())
                                .map(|s| s.as_str())
                                .unwrap_or("unknown error")
                        );

                        let severity = if directive.warn_only {
                            miette::Severity::Warning
                        } else {
                            miette::Severity::Error
                        };

                        result.diagnostics.push(KdlDiagnostic {
                            input: Arc::new(String::new()),
                            #[cfg(feature = "span")]
                            span: directive.span,
                            #[cfg(not(feature = "span"))]
                            span: miette::SourceSpan::new(0.into(), 0),
                            message: Some(msg),
                            label: Some("schema directive".into()),
                            help: Some("Check that the schema file exists and is valid".into()),
                            severity,
                        });
                        result
                            .failed_schemas
                            .push((schema_path, directive.path.clone()));
                    }
                }
            }
        }
        SchemaSource::None => {
            // No directives - nothing to validate
        }
    }

    result
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

    // Tests for spec compliance: about, format type checking, metadata_id

    #[test]
    fn test_about_property_in_error() {
        let schema_src = r#"
document {
    node "person" about="A person node representing an individual" {
        required
    }
}
"#;
        let doc_src = "other"; // missing required "person"
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(!errors.is_empty());
        let msg = errors[0].message.as_ref().unwrap();
        assert!(
            msg.contains("A person node representing"),
            "Error should include about text: {}",
            msg
        );
    }

    #[test]
    fn test_about_child_node_takes_precedence() {
        let schema_src = r#"
document {
    node "person" about="This is the PROPERTY description" {
        required
        about "This is the CHILD NODE description which takes precedence"
    }
}
"#;
        let doc_src = "other"; // missing required "person"
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(!errors.is_empty());
        let msg = errors[0].message.as_ref().unwrap();
        // Child node description should be used, not property
        assert!(
            msg.contains("CHILD NODE description"),
            "Error should use child node about text: {}",
            msg
        );
        assert!(
            !msg.contains("PROPERTY description"),
            "Error should NOT use property about text: {}",
            msg
        );
    }

    #[test]
    fn test_about_in_arg_error() {
        let schema_src = r#"
document {
    node "config" {
        arg about="The configuration value to set" {
            type string
        }
    }
}
"#;
        let doc_src = "config"; // missing required arg
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(!errors.is_empty());
        let msg = errors[0].message.as_ref().unwrap();
        assert!(
            msg.contains("configuration value"),
            "Error should include about text from arg: {}",
            msg
        );
    }

    #[test]
    fn test_about_in_prop_error() {
        let schema_src = r#"
document {
    node "user" {
        prop "name" about="The user's display name" {
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
        let msg = errors[0].message.as_ref().unwrap();
        assert!(
            msg.contains("display name"),
            "Error should include about text from prop: {}",
            msg
        );
    }

    #[test]
    fn test_format_skips_incompatible_type() {
        // Per spec: format validation MUST be skipped if value type doesn't match
        let schema_src = r#"
document {
    node "test" {
        arg {
            type number
            format url  // url format is for strings, should be skipped for numbers
        }
    }
}
"#;
        let doc_src = "test 42"; // number, not string - format should be skipped
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        // Should be valid - format is skipped because type doesn't match
        assert!(
            errors.is_empty(),
            "Expected no errors - format should be skipped for incompatible type: {:?}",
            errors
        );
    }

    #[test]
    fn test_format_applied_for_compatible_type() {
        // Format should be applied when type matches
        let schema_src = r#"
document {
    node "test" {
        arg {
            type string
            format url
        }
    }
}
"#;
        let doc_src = r#"test "not-a-url""#; // string but invalid URL
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        // Should error - format is applied because type matches
        assert!(
            !errors.is_empty(),
            "Expected error - format should be applied for compatible type"
        );
    }

    #[test]
    fn test_integer_format_skips_string() {
        // Integer formats (i8, u32, etc.) should be skipped for strings
        let schema_src = r#"
document {
    node "test" {
        arg {
            type string
            format i32  // i32 format is for integers, should be skipped for strings
        }
    }
}
"#;
        let doc_src = r#"test "hello""#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();
        let errors = schema.validate(&doc);
        assert!(
            errors.is_empty(),
            "Expected no errors - integer format should be skipped for string: {:?}",
            errors
        );
    }

    #[test]
    fn test_metadata_id() {
        let schema_src = r#"
metadata {
    id "https://example.com/schemas/test.kdl"
    title "Test Schema"
}

document {
    node "test"
}
"#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        assert_eq!(
            schema.metadata_id(),
            Some("https://example.com/schemas/test.kdl")
        );
    }

    #[test]
    fn test_metadata_id_missing() {
        let schema_src = r#"
metadata {
    title "Test Schema"
}

document {
    node "test"
}
"#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        assert_eq!(schema.metadata_id(), None);
    }

    #[test]
    fn test_metadata_id_no_metadata() {
        let schema_src = r#"
document {
    node "test"
}
"#;
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        assert_eq!(schema.metadata_id(), None);
    }

    // Tests for directive extraction and path resolution

    #[test]
    fn test_extract_single_directive() {
        let doc_src = r#"
@ksl:schema "schema.kdl"
test
"#;
        let doc: KdlDocument = doc_src.parse().unwrap();
        let source = KdlSchemaV2::extract_directives(&doc);

        match source {
            SchemaSource::Directives(directives) => {
                assert_eq!(directives.len(), 1);
                assert_eq!(directives[0].path, "schema.kdl");
                assert!(!directives[0].warn_only);
            }
            SchemaSource::None => panic!("Expected Directives"),
        }
    }

    #[test]
    fn test_extract_multiple_directives() {
        let doc_src = r#"
@ksl:schema "schema1.kdl"
@ksl:schema "schema2.kdl"
test
"#;
        let doc: KdlDocument = doc_src.parse().unwrap();
        let source = KdlSchemaV2::extract_directives(&doc);

        match source {
            SchemaSource::Directives(directives) => {
                assert_eq!(directives.len(), 2);
                assert_eq!(directives[0].path, "schema1.kdl");
                assert_eq!(directives[1].path, "schema2.kdl");
            }
            SchemaSource::None => panic!("Expected Directives"),
        }
    }

    #[test]
    fn test_extract_multiple_paths() {
        let doc_src = r#"
@ksl:schema "schema1.kdl" "schema2.kdl"
test
"#;
        let doc: KdlDocument = doc_src.parse().unwrap();
        let source = KdlSchemaV2::extract_directives(&doc);

        match source {
            SchemaSource::Directives(directives) => {
                assert_eq!(directives.len(), 2);
                assert_eq!(directives[0].path, "schema1.kdl");
                assert_eq!(directives[1].path, "schema2.kdl");
            }
            SchemaSource::None => panic!("Expected Directives"),
        }
    }

    #[test]
    fn test_extract_warn_only() {
        let doc_src = r#"
@ksl:schema "schema.kdl" warn-only=#true
test
"#;
        let doc: KdlDocument = doc_src.parse().unwrap();
        let source = KdlSchemaV2::extract_directives(&doc);

        match source {
            SchemaSource::Directives(directives) => {
                assert_eq!(directives.len(), 1);
                assert_eq!(directives[0].path, "schema.kdl");
                assert!(directives[0].warn_only);
            }
            SchemaSource::None => panic!("Expected Directives"),
        }
    }

    #[test]
    fn test_extract_no_directive() {
        let doc_src = "test";
        let doc: KdlDocument = doc_src.parse().unwrap();
        let source = KdlSchemaV2::extract_directives(&doc);

        assert!(matches!(source, SchemaSource::None));
    }

    #[test]
    fn test_validate_with_severity_warn_only() {
        let schema_src = r#"
document {
    node "required-node" {
        required
    }
}
"#;
        let doc_src = "other-node"; // missing required node
        let schema = KdlSchemaV2::parse(schema_src).unwrap();
        let doc: KdlDocument = doc_src.parse().unwrap();

        // Without warn_only, should be error
        let errors = schema.validate(&doc);
        assert!(!errors.is_empty());
        assert_eq!(errors[0].severity, miette::Severity::Error);

        // With warn_only, should be warning
        let warnings = schema.validate_with_severity(&doc, true);
        assert!(!warnings.is_empty());
        assert_eq!(warnings[0].severity, miette::Severity::Warning);
    }

    #[test]
    fn test_resolve_absolute_path() {
        let path = resolve_schema_path("/absolute/path/schema.kdl", Path::new("/some/dir"));
        assert_eq!(path, PathBuf::from("/absolute/path/schema.kdl"));
    }

    #[test]
    fn test_resolve_relative_path() {
        let path = resolve_schema_path("schemas/test.kdl", Path::new("/project/src"));
        // Note: canonicalize may fail if path doesn't exist, so we get the joined path
        assert!(path.ends_with("schemas/test.kdl"));
        assert!(path.starts_with("/project/src"));
    }

    #[test]
    fn test_load_from_path_success() {
        let temp_dir = std::env::temp_dir();
        let schema_path = temp_dir.join("test_schema_load.kdl");

        let schema_content = r#"
document {
    node "test"
}
"#;
        std::fs::write(&schema_path, schema_content).unwrap();

        let schema = KdlSchemaV2::load_from_path(&schema_path);
        std::fs::remove_file(&schema_path).ok();

        assert!(schema.is_ok());
        assert!(schema.unwrap().document_node().is_some());
    }

    #[test]
    fn test_load_from_path_not_found() {
        let path = PathBuf::from("/nonexistent/path/schema.kdl");
        let result = KdlSchemaV2::load_from_path(&path);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(!err.diagnostics.is_empty());
        assert!(err.diagnostics[0]
            .message
            .as_ref()
            .unwrap()
            .contains("Failed to read"));
    }

    #[test]
    fn test_load_from_path_invalid_schema() {
        let temp_dir = std::env::temp_dir();
        let schema_path = temp_dir.join("test_invalid_schema.kdl");

        // Valid KDL but not a valid schema (missing document node)
        let invalid_content = "node-without-document";
        std::fs::write(&schema_path, invalid_content).unwrap();

        let result = KdlSchemaV2::load_from_path(&schema_path);
        std::fs::remove_file(&schema_path).ok();

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.diagnostics[0]
            .message
            .as_ref()
            .unwrap()
            .contains("document"));
    }

    #[test]
    fn test_validate_with_directives_no_directives() {
        let doc_src = "some-node";
        let doc: KdlDocument = doc_src.parse().unwrap();

        let result = validate_with_directives(&doc, Path::new("/some/dir"));

        assert!(result.diagnostics.is_empty());
        assert!(result.validated_schemas.is_empty());
        assert!(result.failed_schemas.is_empty());
    }

    #[test]
    fn test_validate_with_directives_missing_schema() {
        let doc_src = r#"
@ksl:schema "nonexistent.kdl"
test-node
"#;
        let doc: KdlDocument = doc_src.parse().unwrap();

        let result = validate_with_directives(&doc, Path::new("/nonexistent/dir"));

        assert!(!result.diagnostics.is_empty());
        assert!(result.validated_schemas.is_empty());
        assert_eq!(result.failed_schemas.len(), 1);
        assert!(result.diagnostics[0]
            .message
            .as_ref()
            .unwrap()
            .contains("Failed to load"));
        assert_eq!(result.diagnostics[0].severity, miette::Severity::Error);
    }

    #[test]
    fn test_validate_with_directives_warn_only_missing() {
        let doc_src = r#"
@ksl:schema "nonexistent.kdl" warn-only=#true
test-node
"#;
        let doc: KdlDocument = doc_src.parse().unwrap();

        let result = validate_with_directives(&doc, Path::new("/nonexistent/dir"));

        assert!(!result.diagnostics.is_empty());
        // Should be warning, not error
        assert_eq!(result.diagnostics[0].severity, miette::Severity::Warning);
    }

    #[test]
    fn test_validate_with_directives_success() {
        let temp_dir = std::env::temp_dir();
        let schema_path = temp_dir.join("test_validate_directive.kdl");

        let schema_content = r#"
document {
    node "test-node"
}
"#;
        std::fs::write(&schema_path, schema_content).unwrap();

        let doc_src = format!(
            r#"
@ksl:schema "{}"
test-node
"#,
            schema_path.display()
        );
        let doc: KdlDocument = doc_src.parse().unwrap();

        let result = validate_with_directives(&doc, &temp_dir);
        std::fs::remove_file(&schema_path).ok();

        assert!(
            result.diagnostics.is_empty(),
            "Expected no errors, got: {:?}",
            result.diagnostics
        );
        assert_eq!(result.validated_schemas.len(), 1);
        assert!(result.failed_schemas.is_empty());
    }

    #[test]
    fn test_validate_with_directives_validation_error() {
        let temp_dir = std::env::temp_dir();
        let schema_path = temp_dir.join("test_validate_error.kdl");

        let schema_content = r#"
document {
    node "required-node" {
        required
    }
}
"#;
        std::fs::write(&schema_path, schema_content).unwrap();

        let doc_src = format!(
            r#"
@ksl:schema "{}"
other-node
"#,
            schema_path.display()
        );
        let doc: KdlDocument = doc_src.parse().unwrap();

        let result = validate_with_directives(&doc, &temp_dir);
        std::fs::remove_file(&schema_path).ok();

        assert!(!result.diagnostics.is_empty());
        // Schema loaded successfully but validation failed
        assert_eq!(result.validated_schemas.len(), 1);
        assert!(result.failed_schemas.is_empty());
    }

    #[test]
    fn test_validate_with_directives_multiple_schemas() {
        let temp_dir = std::env::temp_dir();
        let schema1_path = temp_dir.join("test_multi_schema1.kdl");
        let schema2_path = temp_dir.join("test_multi_schema2.kdl");

        let schema1_content = r#"
document {
    node "node-a"
}
"#;
        let schema2_content = r#"
document {
    node "node-b"
}
"#;
        std::fs::write(&schema1_path, schema1_content).unwrap();
        std::fs::write(&schema2_path, schema2_content).unwrap();

        let doc_src = format!(
            r#"
@ksl:schema "{}"
@ksl:schema "{}"
node-a
node-b
"#,
            schema1_path.display(),
            schema2_path.display()
        );
        let doc: KdlDocument = doc_src.parse().unwrap();

        let result = validate_with_directives(&doc, &temp_dir);
        std::fs::remove_file(&schema1_path).ok();
        std::fs::remove_file(&schema2_path).ok();

        assert!(
            result.diagnostics.is_empty(),
            "Expected no errors, got: {:?}",
            result.diagnostics
        );
        assert_eq!(result.validated_schemas.len(), 2);
        assert!(result.failed_schemas.is_empty());
    }
}
