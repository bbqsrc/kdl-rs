//! Types for schema v2 validation constraints.

use std::sync::Arc;

use miette::Severity;
#[cfg(feature = "span")]
use miette::SourceSpan;
use regex::Regex;

use crate::{KdlDiagnostic, KdlNode, KdlValue};

use jiff::civil::{Date, Time};
use jiff::Span;

/// Format constraints for values as defined in the KDL Schema spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueFormat {
    // String formats
    /// ISO8601 date/time format
    DateTime,
    /// ISO8601 time format
    Time,
    /// ISO8601 date format
    Date,
    /// ISO8601 duration format
    Duration,
    /// IEEE 754-2008 decimal string format
    Decimal,
    /// ISO 4217 currency code
    Currency,
    /// ISO 3166-1 alpha-2 country code
    Country2,
    /// ISO 3166-1 alpha-3 country code
    Country3,
    /// ISO 3166-2 country subdivision code
    CountrySubdivision,
    /// RFC5302 email address
    Email,
    /// RFC6531 internationalized email address
    IdnEmail,
    /// RFC1123 internet hostname
    Hostname,
    /// RFC5890 internationalized internet hostname
    IdnHostname,
    /// RFC2673 dotted-quad IPv4 address
    Ipv4,
    /// RFC2373 IPv6 address
    Ipv6,
    /// RFC3986 URI
    Url,
    /// RFC3986 URI Reference
    UrlReference,
    /// RFC3987 Internationalized Resource Identifier
    Irl,
    /// RFC3987 Internationalized Resource Identifier Reference
    IrlReference,
    /// RFC6570 URI Template
    UrlTemplate,
    /// RFC4122 UUID
    Uuid,
    /// Regular expression
    Regex,
    /// Base64-encoded binary data
    Base64,
    /// KDL Query string
    KdlQuery,
    /// KPath reference (v2 only)
    KPath,

    // Numeric formats
    /// 8-bit signed integer
    I8,
    /// 16-bit signed integer
    I16,
    /// 32-bit signed integer
    I32,
    /// 64-bit signed integer
    I64,
    /// 128-bit signed integer
    I128,
    /// 8-bit unsigned integer
    U8,
    /// 16-bit unsigned integer
    U16,
    /// 32-bit unsigned integer
    U32,
    /// 64-bit unsigned integer
    U64,
    /// 128-bit unsigned integer
    U128,
    /// Platform-dependent signed integer
    Isize,
    /// Platform-dependent unsigned integer
    Usize,
    /// IEEE 754 single precision float
    F32,
    /// IEEE 754 double precision float
    F64,
    /// IEEE 754-2008 64-bit decimal
    Decimal64,
    /// IEEE 754-2008 128-bit decimal
    Decimal128,

    /// Custom/unknown format
    Custom(String),
}

impl ValueFormat {
    /// Parse a format string into a ValueFormat.
    pub fn parse(s: &str) -> Self {
        match s {
            "date-time" => Self::DateTime,
            "time" => Self::Time,
            "date" => Self::Date,
            "duration" => Self::Duration,
            "decimal" => Self::Decimal,
            "currency" => Self::Currency,
            "country-2" => Self::Country2,
            "country-3" => Self::Country3,
            "country-subdivision" => Self::CountrySubdivision,
            "email" => Self::Email,
            "idn-email" => Self::IdnEmail,
            "hostname" => Self::Hostname,
            "idn-hostname" => Self::IdnHostname,
            "ipv4" => Self::Ipv4,
            "ipv6" => Self::Ipv6,
            "url" => Self::Url,
            "url-reference" => Self::UrlReference,
            "irl" => Self::Irl,
            "irl-reference" => Self::IrlReference,
            "url-template" => Self::UrlTemplate,
            "uuid" => Self::Uuid,
            "regex" => Self::Regex,
            "base64" => Self::Base64,
            "kdl-query" => Self::KdlQuery,
            "kpath" => Self::KPath,
            "i8" => Self::I8,
            "i16" => Self::I16,
            "i32" => Self::I32,
            "i64" => Self::I64,
            "i128" => Self::I128,
            "u8" => Self::U8,
            "u16" => Self::U16,
            "u32" => Self::U32,
            "u64" => Self::U64,
            "u128" => Self::U128,
            "isize" => Self::Isize,
            "usize" => Self::Usize,
            "f32" => Self::F32,
            "f64" => Self::F64,
            "decimal64" => Self::Decimal64,
            "decimal128" => Self::Decimal128,
            other => Self::Custom(other.to_string()),
        }
    }
}

/// Compiled validation constraints extracted from a schema node.
#[derive(Debug, Default, Clone)]
pub(super) struct Validations {
    /// Type constraint (e.g., "string", "number", "boolean", "integer")
    pub ty: Option<String>,
    /// Allowed enum values
    pub enum_values: Vec<KdlValue>,
    /// Whether other enum values are allowed (disallow-others=#false on enum)
    pub enum_allow_others: bool,
    /// Pre-compiled regex patterns
    pub patterns: Vec<Regex>,
    /// Minimum string length
    pub min_length: Option<u64>,
    /// Maximum string length
    pub max_length: Option<u64>,
    /// Format constraints (v2: multiple formats as args)
    pub formats: Vec<ValueFormat>,
    /// Multiple-of constraint (%)
    pub multiple_of: Vec<f64>,
    /// Greater than (gt in v2, was >)
    pub gt: Option<f64>,
    /// Greater than or equal (gte in v2, was >=)
    pub gte: Option<f64>,
    /// Less than (lt in v2, was <)
    pub lt: Option<f64>,
    /// Less than or equal (lte in v2, was <=)
    pub lte: Option<f64>,
    /// Tag (type annotation) constraints for values
    pub tag: Option<Box<Validations>>,
    /// Default value
    pub default: Option<KdlValue>,
}

impl Validations {
    /// Extract validation constraints from a schema node's children.
    /// This uses v2 syntax (gte/gt/lte/lt instead of >=/>/<=/<)
    pub(super) fn from_node(node: &KdlNode) -> Self {
        let mut validations = Self::default();

        let Some(children) = node.children() else {
            return validations;
        };

        // type constraint - in v2, type is an identifier, not a string
        // e.g., `type string` instead of `type "string"`
        if let Some(type_node) = children.get("type") {
            // Try as string first (quoted), then as identifier (unquoted)
            if let Some(ty) = type_node.get(0).and_then(|v| v.as_string()) {
                validations.ty = Some(ty.to_string());
            } else if let Some(ty) = type_node.get(0) {
                // For identifiers parsed as other types, get the repr
                validations.ty = Some(format!("{}", ty));
            }
        }

        // enum constraint
        if let Some(enum_node) = children.get("enum") {
            for entry in enum_node.entries() {
                if entry.name().is_none() {
                    validations.enum_values.push(entry.value().clone());
                } else if entry.name().map(|n| n.value()) == Some("disallow-others") {
                    if let Some(b) = entry.value().as_bool() {
                        validations.enum_allow_others = !b;
                    }
                }
            }
        }

        // pattern constraint
        for pnode in children.nodes() {
            if pnode.name().value() == "pattern" {
                if let Some(pattern) = pnode.get(0).and_then(|v| v.as_string()) {
                    if let Ok(regex) = Regex::new(pattern) {
                        validations.patterns.push(regex);
                    }
                }
            }
        }

        // min-length constraint
        if let Some(min_len_node) = children.get("min-length") {
            if let Some(v) = min_len_node.get(0).and_then(|v| v.as_integer()) {
                validations.min_length = Some(v as u64);
            }
        }

        // max-length constraint
        if let Some(max_len_node) = children.get("max-length") {
            if let Some(v) = max_len_node.get(0).and_then(|v| v.as_integer()) {
                validations.max_length = Some(v as u64);
            }
        }

        // format constraint - v2: multiple formats as positional args
        // e.g., `format url irl` instead of `format "url"`
        for fnode in children.nodes() {
            if fnode.name().value() == "format" {
                for entry in fnode.entries() {
                    if entry.name().is_none() {
                        if let Some(fmt_str) = entry.value().as_string() {
                            validations.formats.push(ValueFormat::parse(fmt_str));
                        } else {
                            // Handle unquoted identifiers
                            let fmt_str = format!("{}", entry.value());
                            validations.formats.push(ValueFormat::parse(&fmt_str));
                        }
                    }
                }
            }
        }

        // % (multiple-of) constraint
        for mnode in children.nodes() {
            if mnode.name().value() == "%" {
                if let Some(v) = mnode.get(0).and_then(|v| v.as_float()) {
                    validations.multiple_of.push(v);
                } else if let Some(v) = mnode.get(0).and_then(|v| v.as_integer()) {
                    validations.multiple_of.push(v as f64);
                }
            }
        }

        // gt constraint (v2 name for >)
        if let Some(gt_node) = children.get("gt") {
            if let Some(v) = gt_node.get(0).and_then(|v| v.as_float()) {
                validations.gt = Some(v);
            } else if let Some(v) = gt_node.get(0).and_then(|v| v.as_integer()) {
                validations.gt = Some(v as f64);
            }
        }

        // gte constraint (v2 name for >=)
        if let Some(gte_node) = children.get("gte") {
            if let Some(v) = gte_node.get(0).and_then(|v| v.as_float()) {
                validations.gte = Some(v);
            } else if let Some(v) = gte_node.get(0).and_then(|v| v.as_integer()) {
                validations.gte = Some(v as f64);
            }
        }

        // lt constraint (v2 name for <)
        if let Some(lt_node) = children.get("lt") {
            if let Some(v) = lt_node.get(0).and_then(|v| v.as_float()) {
                validations.lt = Some(v);
            } else if let Some(v) = lt_node.get(0).and_then(|v| v.as_integer()) {
                validations.lt = Some(v as f64);
            }
        }

        // lte constraint (v2 name for <=)
        if let Some(lte_node) = children.get("lte") {
            if let Some(v) = lte_node.get(0).and_then(|v| v.as_float()) {
                validations.lte = Some(v);
            } else if let Some(v) = lte_node.get(0).and_then(|v| v.as_integer()) {
                validations.lte = Some(v as f64);
            }
        }

        // tag constraint (for value type annotations) - now called "annotations" in v2
        if let Some(tag_node) = children.get("annotations") {
            validations.tag = Some(Box::new(Validations::from_node(tag_node)));
        }
        // Also support "tag" for backward compatibility during transition
        if validations.tag.is_none() {
            if let Some(tag_node) = children.get("tag") {
                validations.tag = Some(Box::new(Validations::from_node(tag_node)));
            }
        }

        // default value
        if let Some(default_node) = children.get("default") {
            if let Some(val) = default_node.get(0) {
                validations.default = Some(val.clone());
            }
        }

        validations
    }

    /// Merge constraints from another Validations into this one.
    /// Used when resolving `ref` to combine constraints.
    pub(super) fn merge(&mut self, other: &Validations) {
        if self.ty.is_none() {
            self.ty = other.ty.clone();
        }
        if self.enum_values.is_empty() {
            self.enum_values = other.enum_values.clone();
            self.enum_allow_others = other.enum_allow_others;
        }
        self.patterns.extend(other.patterns.iter().cloned());
        if self.min_length.is_none() {
            self.min_length = other.min_length;
        }
        if self.max_length.is_none() {
            self.max_length = other.max_length;
        }
        self.formats.extend(other.formats.iter().cloned());
        self.multiple_of.extend(other.multiple_of.iter().copied());
        if self.gt.is_none() {
            self.gt = other.gt;
        }
        if self.gte.is_none() {
            self.gte = other.gte;
        }
        if self.lt.is_none() {
            self.lt = other.lt;
        }
        if self.lte.is_none() {
            self.lte = other.lte;
        }
        if self.tag.is_none() {
            self.tag = other.tag.clone();
        }
        if self.default.is_none() {
            self.default = other.default.clone();
        }
    }

    /// Check a value against these validation constraints.
    #[cfg(feature = "span")]
    pub(super) fn check(
        &self,
        value: &KdlValue,
        span: SourceSpan,
        input: &Arc<String>,
    ) -> Vec<KdlDiagnostic> {
        let mut errors = Vec::new();

        // Type check - v2 adds "integer" as distinct from "number"
        if let Some(ref expected_type) = self.ty {
            let actual_type = match value {
                KdlValue::String(_) => "string",
                KdlValue::Integer(_) => "integer",
                KdlValue::Float(_) => "number",
                KdlValue::Bool(_) => "boolean",
                KdlValue::Null => "null",
            };

            // In v2, "number" matches both integer and float
            let type_matches = expected_type == actual_type
                || (expected_type == "number"
                    && (actual_type == "integer" || actual_type == "number"));

            if !type_matches {
                errors.push(KdlDiagnostic {
                    input: input.clone(),
                    span,
                    message: Some(format!(
                        "Expected type '{}', found '{}'",
                        expected_type, actual_type
                    )),
                    label: Some(format!("expected {}", expected_type)),
                    help: None,
                    severity: Severity::Error,
                });
            }
        }

        // Enum check
        if !self.enum_values.is_empty()
            && !self.enum_values.contains(value)
            && !self.enum_allow_others
        {
            errors.push(KdlDiagnostic {
                input: input.clone(),
                span,
                message: Some(format!("Value not in allowed enum: {:?}", self.enum_values)),
                label: Some("invalid value".into()),
                help: None,
                severity: Severity::Error,
            });
        }

        // String-specific checks
        if let KdlValue::String(s) = value {
            // Pattern check
            for pattern in &self.patterns {
                if !pattern.is_match(s) {
                    errors.push(KdlDiagnostic {
                        input: input.clone(),
                        span,
                        message: Some(format!(
                            "Value does not match pattern: {}",
                            pattern.as_str()
                        )),
                        label: Some("pattern mismatch".into()),
                        help: None,
                        severity: Severity::Error,
                    });
                }
            }

            // Length checks
            if let Some(min) = self.min_length {
                if (s.len() as u64) < min {
                    errors.push(KdlDiagnostic {
                        input: input.clone(),
                        span,
                        message: Some(format!(
                            "String length {} is less than minimum {}",
                            s.len(),
                            min
                        )),
                        label: Some("too short".into()),
                        help: None,
                        severity: Severity::Error,
                    });
                }
            }
            if let Some(max) = self.max_length {
                if (s.len() as u64) > max {
                    errors.push(KdlDiagnostic {
                        input: input.clone(),
                        span,
                        message: Some(format!("String length {} exceeds maximum {}", s.len(), max)),
                        label: Some("too long".into()),
                        help: None,
                        severity: Severity::Error,
                    });
                }
            }

            // Format checks - in v2, multiple formats means ANY must match (OR)
            if !self.formats.is_empty() {
                let any_format_matches = self
                    .formats
                    .iter()
                    .any(|format| self.check_string_format(s, format));
                if !any_format_matches {
                    errors.push(KdlDiagnostic {
                        input: input.clone(),
                        span,
                        message: Some(format!(
                            "Value does not match any of formats: {:?}",
                            self.formats
                        )),
                        label: Some("format mismatch".into()),
                        help: None,
                        severity: Severity::Error,
                    });
                }
            }
        }

        // Numeric checks
        let num_value = match value {
            KdlValue::Integer(i) => Some(*i as f64),
            KdlValue::Float(f) => Some(*f),
            _ => None,
        };

        if let Some(num) = num_value {
            // Multiple-of check
            for multiple in &self.multiple_of {
                if (num % multiple).abs() > f64::EPSILON {
                    errors.push(KdlDiagnostic {
                        input: input.clone(),
                        span,
                        message: Some(format!("Value {} is not a multiple of {}", num, multiple)),
                        label: Some("not a multiple".into()),
                        help: None,
                        severity: Severity::Error,
                    });
                }
            }

            // Range checks
            if let Some(gt) = self.gt {
                if num <= gt {
                    errors.push(KdlDiagnostic {
                        input: input.clone(),
                        span,
                        message: Some(format!("Value {} must be greater than {}", num, gt)),
                        label: Some("out of range".into()),
                        help: None,
                        severity: Severity::Error,
                    });
                }
            }
            if let Some(gte) = self.gte {
                if num < gte {
                    errors.push(KdlDiagnostic {
                        input: input.clone(),
                        span,
                        message: Some(format!(
                            "Value {} must be greater than or equal to {}",
                            num, gte
                        )),
                        label: Some("out of range".into()),
                        help: None,
                        severity: Severity::Error,
                    });
                }
            }
            if let Some(lt) = self.lt {
                if num >= lt {
                    errors.push(KdlDiagnostic {
                        input: input.clone(),
                        span,
                        message: Some(format!("Value {} must be less than {}", num, lt)),
                        label: Some("out of range".into()),
                        help: None,
                        severity: Severity::Error,
                    });
                }
            }
            if let Some(lte) = self.lte {
                if num > lte {
                    errors.push(KdlDiagnostic {
                        input: input.clone(),
                        span,
                        message: Some(format!(
                            "Value {} must be less than or equal to {}",
                            num, lte
                        )),
                        label: Some("out of range".into()),
                        help: None,
                        severity: Severity::Error,
                    });
                }
            }

            // Numeric format checks
            if let KdlValue::Integer(i) = value {
                if !self.formats.is_empty() {
                    let any_format_matches = self
                        .formats
                        .iter()
                        .any(|format| self.check_integer_format(*i, format));
                    if !any_format_matches {
                        errors.push(KdlDiagnostic {
                            input: input.clone(),
                            span,
                            message: Some(format!(
                                "Integer does not fit any of formats: {:?}",
                                self.formats
                            )),
                            label: Some("format mismatch".into()),
                            help: None,
                            severity: Severity::Error,
                        });
                    }
                }
            }

            // Float format checks
            if let KdlValue::Float(f) = value {
                if !self.formats.is_empty() {
                    let any_format_matches = self
                        .formats
                        .iter()
                        .any(|format| self.check_float_format(*f, format));
                    if !any_format_matches {
                        errors.push(KdlDiagnostic {
                            input: input.clone(),
                            span,
                            message: Some(format!(
                                "Float does not fit any of formats: {:?}",
                                self.formats
                            )),
                            label: Some("format mismatch".into()),
                            help: None,
                            severity: Severity::Error,
                        });
                    }
                }
            }
        }

        errors
    }

    /// Check if a string matches a format.
    fn check_string_format(&self, s: &str, format: &ValueFormat) -> bool {
        match format {
            ValueFormat::DateTime => {
                s.parse::<jiff::Timestamp>().is_ok() || s.parse::<jiff::civil::DateTime>().is_ok()
            }
            ValueFormat::Date => s.parse::<Date>().is_ok(),
            ValueFormat::Time => s.parse::<Time>().is_ok(),
            ValueFormat::Duration => s.parse::<Span>().is_ok(),
            ValueFormat::Email => s.contains('@') && s.contains('.'),
            ValueFormat::Hostname => {
                !s.is_empty()
                    && s.len() <= 253
                    && s.split('.').all(|label| {
                        !label.is_empty()
                            && label.len() <= 63
                            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                            && !label.starts_with('-')
                            && !label.ends_with('-')
                    })
            }
            ValueFormat::Uuid => {
                s.len() == 36
                    && s.chars().enumerate().all(|(i, c)| match i {
                        8 | 13 | 18 | 23 => c == '-',
                        _ => c.is_ascii_hexdigit(),
                    })
            }
            ValueFormat::Ipv4 => {
                let parts: Vec<&str> = s.split('.').collect();
                parts.len() == 4 && parts.iter().all(|p| p.parse::<u8>().is_ok())
            }
            ValueFormat::Ipv6 => {
                s.contains(':') && s.chars().all(|c| c.is_ascii_hexdigit() || c == ':')
            }
            ValueFormat::Url | ValueFormat::UrlReference => {
                s.contains("://")
                    || s.starts_with('/')
                    || s.starts_with("./")
                    || s.starts_with("../")
            }
            ValueFormat::UrlTemplate => s.contains('{') && s.contains('}'),
            ValueFormat::Currency => s.len() == 3 && s.chars().all(|c| c.is_ascii_uppercase()),
            ValueFormat::Country2 => s.len() == 2 && s.chars().all(|c| c.is_ascii_uppercase()),
            ValueFormat::Country3 => s.len() == 3 && s.chars().all(|c| c.is_ascii_uppercase()),
            ValueFormat::Base64 => s
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='),
            ValueFormat::KPath => s.starts_with('/') || s.starts_with('#'),
            ValueFormat::IdnEmail
            | ValueFormat::IdnHostname
            | ValueFormat::Irl
            | ValueFormat::IrlReference
            | ValueFormat::Decimal
            | ValueFormat::CountrySubdivision
            | ValueFormat::Regex
            | ValueFormat::KdlQuery => true,
            _ => true,
        }
    }

    /// Check if an integer fits a numeric format.
    fn check_integer_format(&self, i: i128, format: &ValueFormat) -> bool {
        match format {
            ValueFormat::I8 => i >= i8::MIN as i128 && i <= i8::MAX as i128,
            ValueFormat::I16 => i >= i16::MIN as i128 && i <= i16::MAX as i128,
            ValueFormat::I32 => i >= i32::MIN as i128 && i <= i32::MAX as i128,
            ValueFormat::I64 => i >= i64::MIN as i128 && i <= i64::MAX as i128,
            ValueFormat::I128 => true,
            ValueFormat::U8 => i >= 0 && i <= u8::MAX as i128,
            ValueFormat::U16 => i >= 0 && i <= u16::MAX as i128,
            ValueFormat::U32 => i >= 0 && i <= u32::MAX as i128,
            ValueFormat::U64 => i >= 0 && i <= u64::MAX as i128,
            ValueFormat::U128 => i >= 0,
            ValueFormat::Isize => i >= isize::MIN as i128 && i <= isize::MAX as i128,
            ValueFormat::Usize => i >= 0 && i <= usize::MAX as i128,
            _ => true,
        }
    }

    /// Check if a float fits a numeric format.
    fn check_float_format(&self, f: f64, format: &ValueFormat) -> bool {
        match format {
            ValueFormat::F32 => f.is_finite() && f >= f32::MIN as f64 && f <= f32::MAX as f64,
            ValueFormat::F64 => true,
            ValueFormat::Decimal64 | ValueFormat::Decimal128 => true,
            _ => true,
        }
    }
}

// Non-span version for when span feature is disabled
#[cfg(not(feature = "span"))]
impl Validations {
    pub(super) fn check(
        &self,
        value: &KdlValue,
        span: miette::SourceSpan,
        input: &Arc<String>,
    ) -> Vec<KdlDiagnostic> {
        // Same implementation but with miette::SourceSpan
        let mut errors = Vec::new();

        if let Some(ref expected_type) = self.ty {
            let actual_type = match value {
                KdlValue::String(_) => "string",
                KdlValue::Integer(_) => "integer",
                KdlValue::Float(_) => "number",
                KdlValue::Bool(_) => "boolean",
                KdlValue::Null => "null",
            };

            let type_matches = expected_type == actual_type
                || (expected_type == "number"
                    && (actual_type == "integer" || actual_type == "number"));

            if !type_matches {
                errors.push(KdlDiagnostic {
                    input: input.clone(),
                    span,
                    message: Some(format!(
                        "Expected type '{}', found '{}'",
                        expected_type, actual_type
                    )),
                    label: Some(format!("expected {}", expected_type)),
                    help: None,
                    severity: Severity::Error,
                });
            }
        }

        if !self.enum_values.is_empty()
            && !self.enum_values.contains(value)
            && !self.enum_allow_others
        {
            errors.push(KdlDiagnostic {
                input: input.clone(),
                span,
                message: Some(format!("Value not in allowed enum: {:?}", self.enum_values)),
                label: Some("invalid value".into()),
                help: None,
                severity: Severity::Error,
            });
        }

        errors
    }
}
