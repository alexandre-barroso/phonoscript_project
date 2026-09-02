//! Exact and explicitly approximate scalar arithmetic.
//!
//! Constraint-based phonology routinely combines integral violation counts,
//! rational weights, finite decimal literals, and exploratory floating-point
//! calculations.  [`NumericScalar`] keeps the epistemic boundary between those
//! domains visible: exact values are arbitrary-precision rationals, whereas an
//! approximate value is finite and carries an [`ApproximationBoundary`].
//!
//! The serialized representation is deliberately verbose.  An exact scalar is
//! stored as canonical numerator and denominator strings; an approximate
//! scalar is stored as a floating-point spelling plus its boundary metadata.
//! Consequently, an `.ottab` reader never has to infer exactness from a JSON
//! number or from the number of written decimal places.

use std::cmp::Ordering;
use std::error::Error;
use std::fmt;
use std::str::FromStr;

use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{One, Signed, ToPrimitive, Zero};
use serde::de;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Default resource limits for exact numeric literals.
///
/// The limits are deliberately generous for analytical work while preventing
/// a short literal such as `1e2000000000` from forcing an unbounded allocation.
pub const DEFAULT_EXACT_LITERAL_LIMITS: ExactLiteralLimits = ExactLiteralLimits {
    max_digits: 10_000,
    max_abs_exponent: 10_000,
};

/// Resource limits applied while parsing an exact integer, rational, decimal,
/// or scientific-notation literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExactLiteralLimits {
    max_digits: usize,
    max_abs_exponent: u32,
}

impl ExactLiteralLimits {
    /// Construct validated literal limits.
    pub fn new(max_digits: usize, max_abs_exponent: u32) -> Result<Self, ScalarError> {
        if max_digits == 0 {
            return Err(ScalarError::new(
                ScalarErrorKind::InvalidLimit,
                "the exact-literal digit limit must be greater than zero",
                "choose a positive maximum digit count",
            ));
        }
        if max_abs_exponent == 0 {
            return Err(ScalarError::new(
                ScalarErrorKind::InvalidLimit,
                "the exact-literal exponent limit must be greater than zero",
                "choose a positive maximum absolute exponent",
            ));
        }
        Ok(Self {
            max_digits,
            max_abs_exponent,
        })
    }

    pub const fn max_digits(self) -> usize {
        self.max_digits
    }

    pub const fn max_abs_exponent(self) -> u32 {
        self.max_abs_exponent
    }

    fn validate(self) -> Result<Self, ScalarError> {
        Self::new(self.max_digits, self.max_abs_exponent)
    }
}

impl Default for ExactLiteralLimits {
    fn default() -> Self {
        DEFAULT_EXACT_LITERAL_LIMITS
    }
}

/// The declared origin of an approximation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "label", rename_all = "snake_case")]
pub enum ApproximationMethod {
    /// An IEEE-754 binary floating-point calculation.
    BinaryFloatingPoint,
    /// A numerical optimization routine.
    NumericalOptimization,
    /// Evaluation on a declared finite grid.
    Grid,
    /// A stochastic or deterministic simulation.
    Simulation,
    /// An approximate value imported from another program or publication.
    Imported,
    /// Approximate arithmetic performed on one or more approximate operands.
    PropagatedArithmetic,
    /// A user-declared method not covered by the stable built-in vocabulary.
    UserDeclared(String),
}

impl ApproximationMethod {
    fn validate(&self) -> Result<(), ScalarError> {
        if let Self::UserDeclared(label) = self
            && label.trim().is_empty()
        {
            return Err(ScalarError::new(
                ScalarErrorKind::InvalidBoundary,
                "a user-declared approximation method needs a non-empty label",
                "name the numerical method or use a built-in approximation method",
            ));
        }
        Ok(())
    }
}

/// Metadata that makes the limit of an approximate value inspectable.
///
/// `certified_absolute_error`, when present, means that the mathematical value
/// lies in `[value - error, value + error]`.  Absence means *uncertified*, not
/// zero error.  `precision_bits` records working precision rather than an error
/// certificate.  The optional source and note are provenance, not proof.
#[derive(Debug, Clone, PartialEq)]
pub struct ApproximationBoundary {
    method: ApproximationMethod,
    precision_bits: Option<u32>,
    certified_absolute_error: Option<f64>,
    source: Option<String>,
    note: Option<String>,
    propagated_by: Option<ArithmeticOperation>,
}

impl ApproximationBoundary {
    /// Construct boundary metadata for a declared approximation method.
    pub fn new(method: ApproximationMethod) -> Result<Self, ScalarError> {
        method.validate()?;
        Ok(Self {
            method,
            precision_bits: None,
            certified_absolute_error: None,
            source: None,
            note: None,
            propagated_by: None,
        })
    }

    /// Boundary for an ordinary finite IEEE-754 `f64` calculation.
    pub fn binary_f64() -> Self {
        Self {
            method: ApproximationMethod::BinaryFloatingPoint,
            precision_bits: Some(f64::MANTISSA_DIGITS),
            certified_absolute_error: None,
            source: None,
            note: None,
            propagated_by: None,
        }
    }

    pub fn method(&self) -> &ApproximationMethod {
        &self.method
    }

    pub const fn precision_bits(&self) -> Option<u32> {
        self.precision_bits
    }

    pub const fn certified_absolute_error(&self) -> Option<f64> {
        self.certified_absolute_error
    }

    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }

    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    pub const fn propagated_by(&self) -> Option<ArithmeticOperation> {
        self.propagated_by
    }

    pub fn with_precision_bits(mut self, precision_bits: u32) -> Result<Self, ScalarError> {
        if precision_bits == 0 {
            return Err(ScalarError::new(
                ScalarErrorKind::InvalidBoundary,
                "working precision must be greater than zero bits",
                "provide the positive precision used by the numerical method",
            ));
        }
        self.precision_bits = Some(precision_bits);
        Ok(self)
    }

    pub fn with_certified_absolute_error(mut self, error: f64) -> Result<Self, ScalarError> {
        validate_absolute_error(error)?;
        self.certified_absolute_error = Some(normalize_zero(error));
        Ok(self)
    }

    pub fn without_certified_error(mut self) -> Self {
        self.certified_absolute_error = None;
        self
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Result<Self, ScalarError> {
        self.source = Some(validate_nonempty_metadata("source", source.into())?);
        Ok(self)
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Result<Self, ScalarError> {
        self.note = Some(validate_nonempty_metadata("note", note.into())?);
        Ok(self)
    }

    fn validate(&self) -> Result<(), ScalarError> {
        self.method.validate()?;
        if self.precision_bits == Some(0) {
            return Err(ScalarError::new(
                ScalarErrorKind::InvalidBoundary,
                "working precision must be greater than zero bits",
                "correct the serialized approximation boundary",
            ));
        }
        if let Some(error) = self.certified_absolute_error {
            validate_absolute_error(error)?;
        }
        if let Some(source) = &self.source {
            validate_nonempty_metadata("source", source.clone())?;
        }
        if let Some(note) = &self.note {
            validate_nonempty_metadata("note", note.clone())?;
        }
        Ok(())
    }

    fn propagated(
        operation: ArithmeticOperation,
        left: &NumericScalar,
        right: &NumericScalar,
        certified_absolute_error: Option<f64>,
    ) -> Self {
        let precision_bits = [left, right]
            .into_iter()
            .filter_map(|value| {
                value
                    .approximation()
                    .and_then(|v| v.boundary.precision_bits)
            })
            .map(|bits| bits.min(f64::MANTISSA_DIGITS))
            .min()
            .or(Some(f64::MANTISSA_DIGITS));

        let source = common_source(left, right);
        Self {
            method: ApproximationMethod::PropagatedArithmetic,
            precision_bits,
            certified_absolute_error,
            source,
            note: Some("approximation propagated through checked scalar arithmetic".to_owned()),
            propagated_by: Some(operation),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApproximationBoundaryWire {
    method: ApproximationMethod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    precision_bits: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    certified_absolute_error: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    propagated_by: Option<ArithmeticOperation>,
}

impl Serialize for ApproximationBoundary {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let wire = ApproximationBoundaryWire {
            method: self.method.clone(),
            precision_bits: self.precision_bits,
            certified_absolute_error: self.certified_absolute_error,
            source: self.source.clone(),
            note: self.note.clone(),
            propagated_by: self.propagated_by,
        };
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ApproximationBoundary {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ApproximationBoundaryWire::deserialize(deserializer)?;
        let value = Self {
            method: wire.method,
            precision_bits: wire.precision_bits,
            certified_absolute_error: wire.certified_absolute_error,
            source: wire.source,
            note: wire.note,
            propagated_by: wire.propagated_by,
        };
        value.validate().map_err(de::Error::custom)?;
        Ok(value)
    }
}

/// A finite floating-point center with an explicit approximation boundary.
#[derive(Debug, Clone, PartialEq)]
pub struct ApproximateNumber {
    value: f64,
    boundary: ApproximationBoundary,
}

impl ApproximateNumber {
    pub fn new(value: f64, boundary: ApproximationBoundary) -> Result<Self, ScalarError> {
        if !value.is_finite() {
            return Err(ScalarError::new(
                ScalarErrorKind::NonFiniteApproximation,
                "an approximate scalar must have a finite center",
                "replace NaN or infinity with a finite approximation or a structured refusal",
            ));
        }
        boundary.validate()?;
        Ok(Self {
            value: normalize_zero(value),
            boundary,
        })
    }

    pub const fn value(&self) -> f64 {
        self.value
    }

    pub const fn boundary(&self) -> &ApproximationBoundary {
        &self.boundary
    }
}

/// A phonological scalar whose exactness status cannot be inferred or erased.
///
/// Structural `PartialEq` distinguishes exact and approximate representations.
/// Use [`NumericScalar::compare`] for a boundary-aware mathematical comparison.
#[derive(Debug, Clone, PartialEq)]
pub enum NumericScalar {
    Exact(BigRational),
    Approximate(ApproximateNumber),
}

impl Default for NumericScalar {
    fn default() -> Self {
        Self::integer(0)
    }
}

impl NumericScalar {
    pub fn exact(value: BigRational) -> Self {
        Self::Exact(value)
    }

    pub fn integer(value: impl Into<BigInt>) -> Self {
        Self::Exact(BigRational::from_integer(value.into()))
    }

    pub fn rational(
        numerator: impl Into<BigInt>,
        denominator: impl Into<BigInt>,
    ) -> Result<Self, ScalarError> {
        let numerator = numerator.into();
        let denominator = denominator.into();
        if denominator.is_zero() {
            return Err(ScalarError::new(
                ScalarErrorKind::ZeroDenominator,
                "an exact rational cannot have a zero denominator",
                "provide a nonzero denominator",
            ));
        }
        Ok(Self::Exact(BigRational::new(numerator, denominator)))
    }

    pub fn approximate(value: f64, boundary: ApproximationBoundary) -> Result<Self, ScalarError> {
        ApproximateNumber::new(value, boundary).map(Self::Approximate)
    }

    /// Construct an explicitly approximate value produced by a GUI numeric
    /// control. This never reclassifies the edited `f64` as an exact decimal.
    pub fn gui_approximate(value: f64) -> Result<Self, ScalarError> {
        let boundary = ApproximationBoundary::binary_f64()
            .with_source("PhonoScript GUI numeric control")?
            .with_note("value edited through a binary floating-point control")?;
        Self::approximate(value, boundary)
    }

    /// Parse canonical editor text. Unprefixed text is exact; a leading `~`
    /// is explicitly approximate and receives GUI-boundary metadata.
    pub fn parse_editor(text: &str) -> Result<Self, ScalarError> {
        if let Some(approximate) = text.strip_prefix('~') {
            let boundary = ApproximationBoundary::binary_f64()
                .with_source("PhonoScript GUI scalar text editor")?
                .with_note("user explicitly entered an approximate scalar")?;
            Self::parse_approximate(approximate, boundary)
        } else {
            Self::parse_exact(text)
        }
    }

    pub fn parse_exact(literal: &str) -> Result<Self, ScalarError> {
        parse_exact_literal(literal).map(Self::Exact)
    }

    pub fn parse_exact_with_limits(
        literal: &str,
        limits: ExactLiteralLimits,
    ) -> Result<Self, ScalarError> {
        parse_exact_literal_with_limits(literal, limits).map(Self::Exact)
    }

    pub fn parse_approximate(
        literal: &str,
        boundary: ApproximationBoundary,
    ) -> Result<Self, ScalarError> {
        parse_approximate_literal(literal, boundary)
    }

    pub const fn is_exact(&self) -> bool {
        matches!(self, Self::Exact(_))
    }

    pub const fn is_approximate(&self) -> bool {
        matches!(self, Self::Approximate(_))
    }

    pub fn exact_value(&self) -> Result<&BigRational, ScalarError> {
        match self {
            Self::Exact(value) => Ok(value),
            Self::Approximate(_) => Err(approximate_to_exact_error()),
        }
    }

    pub fn into_exact(self) -> Result<BigRational, ScalarError> {
        match self {
            Self::Exact(value) => Ok(value),
            Self::Approximate(_) => Err(approximate_to_exact_error()),
        }
    }

    pub const fn approximation(&self) -> Option<&ApproximateNumber> {
        match self {
            Self::Exact(_) => None,
            Self::Approximate(value) => Some(value),
        }
    }

    /// Convert an exact value to an approximate center under declared metadata.
    ///
    /// The conversion error is calculated against the exact rational and added
    /// to the supplied boundary.  Calling this method on an already approximate
    /// value is rejected so it cannot silently replace earlier provenance.
    pub fn exact_to_approximate(
        &self,
        mut boundary: ApproximationBoundary,
    ) -> Result<Self, ScalarError> {
        let exact = match self {
            Self::Exact(value) => value,
            Self::Approximate(_) => {
                return Err(ScalarError::new(
                    ScalarErrorKind::ApproximationRelabeling,
                    "an approximate scalar cannot be relabeled by an exact-to-approximate conversion",
                    "retain its existing boundary or construct an explicit new approximation",
                ));
            }
        };
        boundary.validate()?;
        let center = exact_to_finite_f64(exact)?;
        let center_rational = f64_as_rational(center);
        let conversion_error = (exact - center_rational).abs();
        let conversion_error =
            rational_nonnegative_to_f64_up(&conversion_error).ok_or_else(|| {
                ScalarError::new(
                    ScalarErrorKind::ConversionOutOfRange,
                    "the exact-to-floating conversion error cannot be represented finitely",
                    "retain the exact rational or use a higher-precision numerical backend",
                )
            })?;
        boundary.certified_absolute_error = Some(
            boundary
                .certified_absolute_error
                .map_or(conversion_error, |prior| prior.max(conversion_error)),
        );
        Self::approximate(center, boundary)
    }

    /// Return the floating center without changing the exactness tag.
    ///
    /// Exact values are converted only when their finite `f64` center does not
    /// overflow or underflow to zero.  Callers still receive an `f64`, never a
    /// scalar falsely tagged as exact.
    pub fn to_f64_center(&self) -> Result<f64, ScalarError> {
        match self {
            Self::Exact(value) => exact_to_finite_f64(value),
            Self::Approximate(value) => Ok(value.value),
        }
    }

    /// Stable, canonical human-readable representation.
    pub fn canonical(&self) -> String {
        match self {
            Self::Exact(value) => canonical_rational(value),
            Self::Approximate(value) => format!("~{}", canonical_f64(value.value)),
        }
    }

    pub fn checked_add(&self, right: &Self) -> Result<Self, ScalarError> {
        self.checked_arithmetic(right, ArithmeticOperation::Add)
    }

    pub fn checked_sub(&self, right: &Self) -> Result<Self, ScalarError> {
        self.checked_arithmetic(right, ArithmeticOperation::Subtract)
    }

    pub fn checked_mul(&self, right: &Self) -> Result<Self, ScalarError> {
        self.checked_arithmetic(right, ArithmeticOperation::Multiply)
    }

    pub fn checked_div(&self, right: &Self) -> Result<Self, ScalarError> {
        self.checked_arithmetic(right, ArithmeticOperation::Divide)
    }

    /// Compare mathematical values only to the strength warranted by their
    /// declared boundaries.
    ///
    /// Exact values use rational ordering.  Approximate values with disjoint
    /// certified intervals can be ordered.  Overlapping or uncertified
    /// intervals are indeterminate; their floating centers are not substituted
    /// for a proof of equality or order.
    pub fn compare(&self, right: &Self) -> ScalarComparison {
        if let (Self::Exact(left), Self::Exact(right)) = (self, right) {
            return ScalarComparison::from(left.cmp(right));
        }

        let Some(left_interval) = self.certified_interval() else {
            return ScalarComparison::Indeterminate(
                ComparisonIndeterminacy::UncertifiedApproximation,
            );
        };
        let Some(right_interval) = right.certified_interval() else {
            return ScalarComparison::Indeterminate(
                ComparisonIndeterminacy::UncertifiedApproximation,
            );
        };

        if left_interval.high < right_interval.low {
            ScalarComparison::Less
        } else if left_interval.low > right_interval.high {
            ScalarComparison::Greater
        } else if left_interval.is_singleton()
            && right_interval.is_singleton()
            && left_interval.low == right_interval.low
        {
            ScalarComparison::Equal
        } else {
            ScalarComparison::Indeterminate(ComparisonIndeterminacy::OverlappingBoundaries)
        }
    }

    /// Exploratory comparison of finite centers.
    ///
    /// This method is intentionally named differently from [`Self::compare`].
    /// It reports floating-center order and supplies no exact or interval
    /// certificate.
    pub fn compare_centers(&self, right: &Self) -> Result<Ordering, ScalarError> {
        let left = self.to_f64_center()?;
        let right = right.to_f64_center()?;
        Ok(left.total_cmp(&right))
    }

    fn checked_arithmetic(
        &self,
        right: &Self,
        operation: ArithmeticOperation,
    ) -> Result<Self, ScalarError> {
        if let (Self::Exact(left), Self::Exact(right)) = (self, right) {
            if operation == ArithmeticOperation::Divide && right.is_zero() {
                return Err(arithmetic_error(
                    ScalarErrorKind::DivisionByZero,
                    operation,
                    "exact division by zero is undefined",
                    "provide a nonzero divisor",
                ));
            }
            return Ok(Self::Exact(match operation {
                ArithmeticOperation::Add => left + right,
                ArithmeticOperation::Subtract => left - right,
                ArithmeticOperation::Multiply => left * right,
                ArithmeticOperation::Divide => left / right,
            }));
        }

        let left_center = self.to_f64_center()?;
        let right_center = right.to_f64_center()?;
        if operation == ArithmeticOperation::Divide && right_center == 0.0 {
            return Err(arithmetic_error(
                ScalarErrorKind::DivisionByZero,
                operation,
                "approximate division has a zero floating-point center",
                "provide a divisor whose center and certified interval exclude zero",
            ));
        }

        let right_interval = right.certified_interval();
        if operation == ArithmeticOperation::Divide
            && right_interval
                .as_ref()
                .is_some_and(RationalInterval::contains_zero)
        {
            return Err(arithmetic_error(
                ScalarErrorKind::UnstableDivisionBoundary,
                operation,
                "the certified divisor interval contains zero",
                "refine the approximation until its certified interval excludes zero",
            ));
        }

        let center = match operation {
            ArithmeticOperation::Add => left_center + right_center,
            ArithmeticOperation::Subtract => left_center - right_center,
            ArithmeticOperation::Multiply => left_center * right_center,
            ArithmeticOperation::Divide => left_center / right_center,
        };
        if !center.is_finite() {
            return Err(arithmetic_error(
                ScalarErrorKind::NonFiniteResult,
                operation,
                "floating-point arithmetic produced NaN or infinity",
                "use exact arithmetic, rescale the analysis, or use a higher-precision backend",
            ));
        }

        let certified_absolute_error = self
            .certified_interval()
            .zip(right_interval)
            .and_then(|(left, right)| left.apply(&right, operation).ok())
            .and_then(|interval| interval.absolute_error_around(center));

        let boundary =
            ApproximationBoundary::propagated(operation, self, right, certified_absolute_error);
        Self::approximate(center, boundary)
    }

    fn certified_interval(&self) -> Option<RationalInterval> {
        match self {
            Self::Exact(value) => Some(RationalInterval::singleton(value.clone())),
            Self::Approximate(value) => {
                let error = value.boundary.certified_absolute_error?;
                let center = f64_as_rational(value.value);
                let error = f64_as_rational(error);
                Some(RationalInterval {
                    low: &center - &error,
                    high: center + error,
                })
            }
        }
    }
}

impl fmt::Display for NumericScalar {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.canonical())
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum NumericScalarSerialize<'a> {
    Exact {
        numerator: String,
        denominator: String,
    },
    Approximate {
        value: String,
        boundary: &'a ApproximationBoundary,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum NumericScalarDeserialize {
    Exact {
        numerator: String,
        denominator: String,
    },
    Approximate {
        value: String,
        boundary: ApproximationBoundary,
    },
}

impl Serialize for NumericScalar {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Exact(value) => NumericScalarSerialize::Exact {
                numerator: value.numer().to_string(),
                denominator: value.denom().to_string(),
            }
            .serialize(serializer),
            Self::Approximate(value) => NumericScalarSerialize::Approximate {
                value: canonical_f64(value.value),
                boundary: &value.boundary,
            }
            .serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for NumericScalar {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match NumericScalarDeserialize::deserialize(deserializer)? {
            NumericScalarDeserialize::Exact {
                numerator,
                denominator,
            } => {
                let written_digits =
                    count_ascii_digits(&numerator).saturating_add(count_ascii_digits(&denominator));
                if written_digits > DEFAULT_EXACT_LITERAL_LIMITS.max_digits {
                    return Err(de::Error::custom(ScalarError::new(
                        ScalarErrorKind::LiteralTooLong,
                        format!(
                            "the serialized exact scalar contains {written_digits} digits, exceeding the safe `.ottab` limit of {}",
                            DEFAULT_EXACT_LITERAL_LIMITS.max_digits
                        ),
                        "shorten the exact scalar before loading the project",
                    )));
                }
                let numerator =
                    parse_wire_integer(&numerator, "numerator").map_err(de::Error::custom)?;
                let denominator =
                    parse_wire_integer(&denominator, "denominator").map_err(de::Error::custom)?;
                Self::rational(numerator, denominator).map_err(de::Error::custom)
            }
            NumericScalarDeserialize::Approximate { value, boundary } => {
                let value = parse_finite_f64(&value).map_err(de::Error::custom)?;
                Self::approximate(value, boundary).map_err(de::Error::custom)
            }
        }
    }
}

/// The operation attached to an arithmetic diagnostic or propagated boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArithmeticOperation {
    Add,
    Subtract,
    Multiply,
    Divide,
}

impl fmt::Display for ArithmeticOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Add => "addition",
            Self::Subtract => "subtraction",
            Self::Multiply => "multiplication",
            Self::Divide => "division",
        })
    }
}

/// Result of a boundary-aware scalar comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "status", content = "reason", rename_all = "snake_case")]
pub enum ScalarComparison {
    Less,
    Equal,
    Greater,
    Indeterminate(ComparisonIndeterminacy),
}

impl From<Ordering> for ScalarComparison {
    fn from(ordering: Ordering) -> Self {
        match ordering {
            Ordering::Less => Self::Less,
            Ordering::Equal => Self::Equal,
            Ordering::Greater => Self::Greater,
        }
    }
}

/// Why an approximate comparison could not establish an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonIndeterminacy {
    /// At least one approximate operand has no certified absolute-error bound.
    UncertifiedApproximation,
    /// Certified intervals intersect and therefore do not determine the order.
    OverlappingBoundaries,
}

/// Stable machine-readable categories for scalar failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScalarErrorKind {
    EmptyLiteral,
    WhitespaceInLiteral,
    InvalidSyntax,
    InvalidDigit,
    MultipleSeparators,
    ZeroDenominator,
    LiteralTooLong,
    ExponentOutOfRange,
    InvalidLimit,
    NonFiniteApproximation,
    InvalidBoundary,
    DivisionByZero,
    NonFiniteResult,
    ConversionOutOfRange,
    ApproximateCannotBecomeExact,
    ApproximationRelabeling,
    UnstableDivisionBoundary,
    InvalidSerializedValue,
}

/// Structured parse, conversion, boundary, and arithmetic diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScalarError {
    pub kind: ScalarErrorKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<ArithmeticOperation>,
    pub message: String,
    pub remedy: String,
}

impl ScalarError {
    pub fn new(
        kind: ScalarErrorKind,
        message: impl Into<String>,
        remedy: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            input: None,
            position: None,
            operation: None,
            message: message.into(),
            remedy: remedy.into(),
        }
    }

    fn with_input(mut self, input: &str) -> Self {
        self.input = Some(input.to_owned());
        self
    }

    fn with_position(mut self, position: usize) -> Self {
        self.position = Some(position);
        self
    }

    fn with_operation(mut self, operation: ArithmeticOperation) -> Self {
        self.operation = Some(operation);
        self
    }
}

impl fmt::Display for ScalarError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)?;
        if let Some(position) = self.position {
            write!(formatter, " at byte {position}")?;
        }
        if let Some(operation) = self.operation {
            write!(formatter, " during {operation}")?;
        }
        write!(formatter, "; {}", self.remedy)
    }
}

impl Error for ScalarError {}

/// Parse an integer, rational, finite decimal, or decimal scientific literal
/// as an arbitrary-precision exact rational.
pub fn parse_exact_literal(literal: &str) -> Result<BigRational, ScalarError> {
    parse_exact_literal_with_limits(literal, DEFAULT_EXACT_LITERAL_LIMITS)
}

/// Parse an exact literal under explicit allocation limits.
pub fn parse_exact_literal_with_limits(
    literal: &str,
    limits: ExactLiteralLimits,
) -> Result<BigRational, ScalarError> {
    let limits = limits.validate()?;
    validate_literal_preamble(literal)?;

    let slash_count = literal.bytes().filter(|byte| *byte == b'/').count();
    if slash_count > 1 {
        return Err(literal_error(
            ScalarErrorKind::MultipleSeparators,
            literal,
            literal.find('/').unwrap_or(0),
            "an exact rational may contain only one slash",
            "write the rational as one integer numerator and one nonzero integer denominator",
        ));
    }
    if slash_count == 1 {
        return parse_rational_literal(literal, limits);
    }
    parse_decimal_literal(literal, limits)
}

/// Parse a floating-point literal as explicitly approximate.
pub fn parse_approximate_literal(
    literal: &str,
    boundary: ApproximationBoundary,
) -> Result<NumericScalar, ScalarError> {
    validate_literal_preamble(literal)?;
    let value = parse_finite_f64(literal).map_err(|error| error.with_input(literal))?;
    NumericScalar::approximate(value, boundary)
}

fn validate_literal_preamble(literal: &str) -> Result<(), ScalarError> {
    if literal.is_empty() {
        return Err(ScalarError::new(
            ScalarErrorKind::EmptyLiteral,
            "a numeric literal cannot be empty",
            "provide an integer, rational, decimal, or scientific literal",
        )
        .with_input(literal));
    }
    if let Some((position, _)) = literal.char_indices().find(|(_, ch)| ch.is_whitespace()) {
        return Err(literal_error(
            ScalarErrorKind::WhitespaceInLiteral,
            literal,
            position,
            "whitespace is not permitted inside a numeric literal",
            "remove surrounding or internal whitespace before parsing",
        ));
    }
    Ok(())
}

fn parse_rational_literal(
    literal: &str,
    limits: ExactLiteralLimits,
) -> Result<BigRational, ScalarError> {
    let (numerator, denominator) = literal.split_once('/').expect("slash count checked");
    let total_digits = count_ascii_digits(numerator) + count_ascii_digits(denominator);
    enforce_digit_limit(literal, total_digits, limits)?;
    let numerator = parse_signed_integer_component(numerator, literal, 0)?;
    let denominator_offset = literal.find('/').expect("slash exists") + 1;
    let denominator = parse_signed_integer_component(denominator, literal, denominator_offset)?;
    if denominator.is_zero() {
        return Err(literal_error(
            ScalarErrorKind::ZeroDenominator,
            literal,
            denominator_offset,
            "an exact rational cannot have a zero denominator",
            "provide a nonzero denominator",
        ));
    }
    Ok(BigRational::new(numerator, denominator))
}

fn parse_signed_integer_component(
    component: &str,
    whole: &str,
    offset: usize,
) -> Result<BigInt, ScalarError> {
    if component.is_empty() || component == "+" || component == "-" {
        return Err(literal_error(
            ScalarErrorKind::InvalidSyntax,
            whole,
            offset,
            "a rational component must contain at least one digit",
            "write both numerator and denominator as signed integers",
        ));
    }
    for (position, byte) in component.bytes().enumerate() {
        let is_sign = position == 0 && matches!(byte, b'+' | b'-');
        if !is_sign && !byte.is_ascii_digit() {
            return Err(literal_error(
                ScalarErrorKind::InvalidDigit,
                whole,
                offset + position,
                "a rational component contains a non-integer character",
                "use decimal digits with an optional leading sign",
            ));
        }
    }
    BigInt::from_str(component).map_err(|_| {
        literal_error(
            ScalarErrorKind::InvalidSyntax,
            whole,
            offset,
            "the integer component could not be parsed",
            "use decimal digits with an optional leading sign",
        )
    })
}

fn parse_decimal_literal(
    literal: &str,
    limits: ExactLiteralLimits,
) -> Result<BigRational, ScalarError> {
    let bytes = literal.as_bytes();
    let mut cursor = 0usize;
    let negative = match bytes.first() {
        Some(b'+') => {
            cursor = 1;
            false
        }
        Some(b'-') => {
            cursor = 1;
            true
        }
        _ => false,
    };

    let mut digits = String::new();
    let mut fractional_digits = 0usize;
    let mut saw_digit = false;
    let mut saw_decimal_point = false;

    while cursor < bytes.len() && !matches!(bytes[cursor], b'e' | b'E') {
        match bytes[cursor] {
            byte if byte.is_ascii_digit() => {
                saw_digit = true;
                digits.push(char::from(byte));
                if saw_decimal_point {
                    fractional_digits += 1;
                }
            }
            b'.' if !saw_decimal_point => saw_decimal_point = true,
            b'.' => {
                return Err(literal_error(
                    ScalarErrorKind::MultipleSeparators,
                    literal,
                    cursor,
                    "a finite decimal may contain only one decimal point",
                    "remove the extra decimal point",
                ));
            }
            _ => {
                return Err(literal_error(
                    ScalarErrorKind::InvalidDigit,
                    literal,
                    cursor,
                    "the exact decimal contains an unsupported character",
                    "use decimal digits, one optional point, and one optional base-ten exponent",
                ));
            }
        }
        cursor += 1;
    }

    if !saw_digit {
        return Err(literal_error(
            ScalarErrorKind::InvalidSyntax,
            literal,
            0,
            "the exact decimal has no significand digits",
            "provide at least one digit before or after the decimal point",
        ));
    }
    enforce_digit_limit(literal, digits.len(), limits)?;

    let exponent = if cursor < bytes.len() {
        let exponent_marker = cursor;
        cursor += 1;
        if cursor >= bytes.len() {
            return Err(literal_error(
                ScalarErrorKind::InvalidSyntax,
                literal,
                exponent_marker,
                "the scientific literal has no exponent digits",
                "write an integer exponent after `e`",
            ));
        }
        let exponent_start = cursor;
        let exponent_negative = match bytes[cursor] {
            b'+' => {
                cursor += 1;
                false
            }
            b'-' => {
                cursor += 1;
                true
            }
            _ => false,
        };
        if cursor >= bytes.len() {
            return Err(literal_error(
                ScalarErrorKind::InvalidSyntax,
                literal,
                exponent_start,
                "the scientific literal has an exponent sign but no digits",
                "write integer digits after the exponent sign",
            ));
        }
        let mut magnitude = 0u32;
        while cursor < bytes.len() {
            let byte = bytes[cursor];
            if !byte.is_ascii_digit() {
                return Err(literal_error(
                    ScalarErrorKind::InvalidDigit,
                    literal,
                    cursor,
                    "the exponent must be an integer",
                    "use decimal digits with one optional leading sign",
                ));
            }
            magnitude = magnitude
                .checked_mul(10)
                .and_then(|value| value.checked_add(u32::from(byte - b'0')))
                .ok_or_else(|| exponent_limit_error(literal, exponent_start, limits))?;
            if magnitude > limits.max_abs_exponent {
                return Err(exponent_limit_error(literal, exponent_start, limits));
            }
            cursor += 1;
        }
        if exponent_negative {
            -i64::from(magnitude)
        } else {
            i64::from(magnitude)
        }
    } else {
        0
    };

    let mut numerator = BigInt::from_str(&digits).expect("digits were validated");
    if negative {
        numerator = -numerator;
    }
    if numerator.is_zero() {
        return Ok(BigRational::zero());
    }

    let fractional_digits = i64::try_from(fractional_digits).map_err(|_| {
        ScalarError::new(
            ScalarErrorKind::LiteralTooLong,
            "the fractional digit count exceeds this platform's parsing range",
            "use a shorter exact literal",
        )
        .with_input(literal)
    })?;
    let scale = fractional_digits.checked_sub(exponent).ok_or_else(|| {
        exponent_limit_error(literal, literal.find(['e', 'E']).unwrap_or(0), limits)
    })?;
    let power = u32::try_from(scale.unsigned_abs()).map_err(|_| {
        exponent_limit_error(literal, literal.find(['e', 'E']).unwrap_or(0), limits)
    })?;
    let ten_power = BigInt::from(10u8).pow(power);
    if scale >= 0 {
        Ok(BigRational::new(numerator, ten_power))
    } else {
        Ok(BigRational::from_integer(numerator * ten_power))
    }
}

fn enforce_digit_limit(
    literal: &str,
    digits: usize,
    limits: ExactLiteralLimits,
) -> Result<(), ScalarError> {
    if digits > limits.max_digits {
        return Err(ScalarError::new(
            ScalarErrorKind::LiteralTooLong,
            format!(
                "the exact literal contains {digits} digits, exceeding the configured limit of {}",
                limits.max_digits
            ),
            "raise the explicit limit only for a trusted analysis or use a shorter literal",
        )
        .with_input(literal));
    }
    Ok(())
}

fn exponent_limit_error(literal: &str, position: usize, limits: ExactLiteralLimits) -> ScalarError {
    literal_error(
        ScalarErrorKind::ExponentOutOfRange,
        literal,
        position,
        format!(
            "the base-ten exponent exceeds the configured absolute limit of {}",
            limits.max_abs_exponent
        ),
        "raise the explicit limit only for a trusted analysis or reduce the exponent",
    )
}

fn count_ascii_digits(component: &str) -> usize {
    component.bytes().filter(u8::is_ascii_digit).count()
}

fn canonical_rational(value: &BigRational) -> String {
    if value.denom().is_one() {
        value.numer().to_string()
    } else {
        format!("{}/{}", value.numer(), value.denom())
    }
}

fn canonical_f64(value: f64) -> String {
    normalize_zero(value).to_string()
}

fn normalize_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

fn parse_wire_integer(value: &str, role: &str) -> Result<BigInt, ScalarError> {
    if value.is_empty()
        || value == "+"
        || value == "-"
        || value
            .bytes()
            .enumerate()
            .any(|(index, byte)| !(byte.is_ascii_digit() || index == 0 && byte == b'-'))
    {
        return Err(ScalarError::new(
            ScalarErrorKind::InvalidSerializedValue,
            format!("the serialized exact {role} is not a canonical integer"),
            "store exact numerator and denominator as base-ten integer strings",
        )
        .with_input(value));
    }
    BigInt::from_str(value).map_err(|_| {
        ScalarError::new(
            ScalarErrorKind::InvalidSerializedValue,
            format!("the serialized exact {role} could not be parsed"),
            "store exact numerator and denominator as base-ten integer strings",
        )
        .with_input(value)
    })
}

fn parse_finite_f64(value: &str) -> Result<f64, ScalarError> {
    let parsed = f64::from_str(value).map_err(|_| {
        ScalarError::new(
            ScalarErrorKind::InvalidSyntax,
            "the approximate literal is not a floating-point number",
            "use a finite decimal or scientific floating-point spelling",
        )
    })?;
    if !parsed.is_finite() {
        return Err(ScalarError::new(
            ScalarErrorKind::NonFiniteApproximation,
            "NaN and infinity are not valid approximate scalar centers",
            "use a finite approximation or a structured refusal",
        ));
    }
    Ok(normalize_zero(parsed))
}

fn exact_to_finite_f64(value: &BigRational) -> Result<f64, ScalarError> {
    let center = value.to_f64().ok_or_else(|| {
        ScalarError::new(
            ScalarErrorKind::ConversionOutOfRange,
            "the exact rational is outside the finite f64 range",
            "retain exact arithmetic or use a higher-precision numerical backend",
        )
    })?;
    if !center.is_finite() || (!value.is_zero() && center == 0.0) {
        return Err(ScalarError::new(
            ScalarErrorKind::ConversionOutOfRange,
            "the exact rational overflows or underflows the finite f64 range",
            "retain exact arithmetic or use a higher-precision numerical backend",
        ));
    }
    Ok(normalize_zero(center))
}

fn f64_as_rational(value: f64) -> BigRational {
    debug_assert!(value.is_finite());
    BigRational::from_float(normalize_zero(value)).expect("finite f64 has an exact binary rational")
}

fn rational_nonnegative_to_f64_up(value: &BigRational) -> Option<f64> {
    debug_assert!(!value.is_negative());
    let rounded = value.to_f64()?;
    if !rounded.is_finite() {
        return None;
    }
    if f64_as_rational(rounded) < *value {
        let upward = rounded.next_up();
        upward.is_finite().then_some(upward)
    } else {
        Some(rounded)
    }
}

fn validate_absolute_error(error: f64) -> Result<(), ScalarError> {
    if !error.is_finite() || error < 0.0 {
        return Err(ScalarError::new(
            ScalarErrorKind::InvalidBoundary,
            "a certified absolute error must be finite and nonnegative",
            "provide a finite error radius greater than or equal to zero",
        ));
    }
    Ok(())
}

fn validate_nonempty_metadata(role: &str, value: String) -> Result<String, ScalarError> {
    if value.trim().is_empty() {
        return Err(ScalarError::new(
            ScalarErrorKind::InvalidBoundary,
            format!("approximation {role} metadata cannot be empty"),
            format!("omit the {role} or provide a non-empty description"),
        ));
    }
    Ok(value)
}

fn common_source(left: &NumericScalar, right: &NumericScalar) -> Option<String> {
    let left = left
        .approximation()
        .and_then(|value| value.boundary.source.as_ref());
    let right = right
        .approximation()
        .and_then(|value| value.boundary.source.as_ref());
    match (left, right) {
        (Some(left), Some(right)) if left == right => Some(left.clone()),
        (Some(source), None) | (None, Some(source)) => Some(source.clone()),
        _ => None,
    }
}

fn approximate_to_exact_error() -> ScalarError {
    ScalarError::new(
        ScalarErrorKind::ApproximateCannotBecomeExact,
        "an approximate scalar cannot be converted to an exact rational",
        "retain the approximate boundary or recompute from exact source data",
    )
}

fn literal_error(
    kind: ScalarErrorKind,
    input: &str,
    position: usize,
    message: impl Into<String>,
    remedy: impl Into<String>,
) -> ScalarError {
    ScalarError::new(kind, message, remedy)
        .with_input(input)
        .with_position(position)
}

fn arithmetic_error(
    kind: ScalarErrorKind,
    operation: ArithmeticOperation,
    message: impl Into<String>,
    remedy: impl Into<String>,
) -> ScalarError {
    ScalarError::new(kind, message, remedy).with_operation(operation)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RationalInterval {
    low: BigRational,
    high: BigRational,
}

impl RationalInterval {
    fn singleton(value: BigRational) -> Self {
        Self {
            low: value.clone(),
            high: value,
        }
    }

    fn is_singleton(&self) -> bool {
        self.low == self.high
    }

    fn contains_zero(&self) -> bool {
        self.low <= BigRational::zero() && self.high >= BigRational::zero()
    }

    fn apply(&self, right: &Self, operation: ArithmeticOperation) -> Result<Self, ScalarError> {
        match operation {
            ArithmeticOperation::Add => Ok(Self {
                low: &self.low + &right.low,
                high: &self.high + &right.high,
            }),
            ArithmeticOperation::Subtract => Ok(Self {
                low: &self.low - &right.high,
                high: &self.high - &right.low,
            }),
            ArithmeticOperation::Multiply => {
                let products = [
                    &self.low * &right.low,
                    &self.low * &right.high,
                    &self.high * &right.low,
                    &self.high * &right.high,
                ];
                Ok(Self::from_values(products))
            }
            ArithmeticOperation::Divide => {
                if right.contains_zero() {
                    return Err(arithmetic_error(
                        ScalarErrorKind::UnstableDivisionBoundary,
                        operation,
                        "the certified divisor interval contains zero",
                        "refine the divisor approximation before division",
                    ));
                }
                let quotients = [
                    &self.low / &right.low,
                    &self.low / &right.high,
                    &self.high / &right.low,
                    &self.high / &right.high,
                ];
                Ok(Self::from_values(quotients))
            }
        }
    }

    fn from_values<const N: usize>(values: [BigRational; N]) -> Self {
        let mut iterator = values.into_iter();
        let first = iterator.next().expect("interval operation has endpoints");
        iterator.fold(
            Self {
                low: first.clone(),
                high: first,
            },
            |mut interval, value| {
                if value < interval.low {
                    interval.low = value.clone();
                }
                if value > interval.high {
                    interval.high = value;
                }
                interval
            },
        )
    }

    fn absolute_error_around(&self, center: f64) -> Option<f64> {
        let center = f64_as_rational(center);
        let low_error = (&center - &self.low).abs();
        let high_error = (&self.high - &center).abs();
        rational_nonnegative_to_f64_up(if low_error >= high_error {
            &low_error
        } else {
            &high_error
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::json;

    fn certified(error: f64) -> ApproximationBoundary {
        ApproximationBoundary::binary_f64()
            .with_certified_absolute_error(error)
            .unwrap()
    }

    #[test]
    fn finite_decimal_is_exact_not_binary_floating_point() {
        let decimal = parse_exact_literal("0.1").unwrap();
        assert_eq!(decimal, BigRational::new(1.into(), 10.into()));
        assert_eq!(canonical_rational(&decimal), "1/10");
    }

    #[test]
    fn exact_parser_accepts_integer_rational_decimal_and_scientific_forms() {
        let cases = [
            ("-42", "-42"),
            ("+6/-8", "-3/4"),
            (".125", "1/8"),
            ("5.", "5"),
            ("-12.50e-2", "-1/8"),
            ("2.5E3", "2500"),
            ("-0.000e900", "0"),
        ];
        for (literal, expected) in cases {
            assert_eq!(
                canonical_rational(&parse_exact_literal(literal).unwrap()),
                expected,
                "literal {literal}"
            );
        }
    }

    #[test]
    fn exact_parser_reports_structured_failures_and_limits() {
        let zero = parse_exact_literal("1/0").unwrap_err();
        assert_eq!(zero.kind, ScalarErrorKind::ZeroDenominator);
        assert_eq!(zero.position, Some(2));

        let limits = ExactLiteralLimits::new(3, 4).unwrap();
        assert_eq!(
            parse_exact_literal_with_limits("1234", limits)
                .unwrap_err()
                .kind,
            ScalarErrorKind::LiteralTooLong
        );
        assert_eq!(
            parse_exact_literal_with_limits("1e5", limits)
                .unwrap_err()
                .kind,
            ScalarErrorKind::ExponentOutOfRange
        );
        assert_eq!(
            parse_exact_literal("1 0").unwrap_err().kind,
            ScalarErrorKind::WhitespaceInLiteral
        );
    }

    #[test]
    fn canonical_rendering_marks_approximation() {
        let exact = NumericScalar::parse_exact("6/8").unwrap();
        let approximate = NumericScalar::approximate(-0.0, certified(0.0)).unwrap();
        assert_eq!(exact.canonical(), "3/4");
        assert_eq!(approximate.canonical(), "~0");
    }

    #[test]
    fn ottab_serde_is_explicit_and_round_trips() {
        let exact = NumericScalar::parse_exact("0.125").unwrap();
        let exact_json = serde_json::to_value(&exact).unwrap();
        assert_eq!(
            exact_json,
            json!({"kind":"exact", "numerator":"1", "denominator":"8"})
        );
        assert_eq!(
            serde_json::from_value::<NumericScalar>(exact_json).unwrap(),
            exact
        );

        let boundary = ApproximationBoundary::new(ApproximationMethod::NumericalOptimization)
            .unwrap()
            .with_precision_bits(48)
            .unwrap()
            .with_certified_absolute_error(1e-9)
            .unwrap()
            .with_source("continuous-HG optimizer")
            .unwrap();
        let approximate = NumericScalar::approximate(0.375, boundary).unwrap();
        let approximate_json = serde_json::to_value(&approximate).unwrap();
        assert_eq!(approximate_json["kind"], "approximate");
        assert_eq!(approximate_json["value"], "0.375");
        assert_eq!(
            approximate_json["boundary"]["method"]["kind"],
            "numerical_optimization"
        );
        assert_eq!(
            serde_json::from_value::<NumericScalar>(approximate_json).unwrap(),
            approximate
        );
    }

    #[test]
    fn deserialization_rejects_nonfinite_or_invalid_boundaries() {
        let nonfinite = json!({
            "kind": "approximate",
            "value": "NaN",
            "boundary": {"method": {"kind": "binary_floating_point"}}
        });
        assert!(serde_json::from_value::<NumericScalar>(nonfinite).is_err());

        let invalid_error = json!({
            "kind": "approximate",
            "value": "1.0",
            "boundary": {
                "method": {"kind": "binary_floating_point"},
                "certified_absolute_error": -0.5
            }
        });
        assert!(serde_json::from_value::<NumericScalar>(invalid_error).is_err());

        let zero_denominator = json!({
            "kind": "exact",
            "numerator": "1",
            "denominator": "0"
        });
        assert!(serde_json::from_value::<NumericScalar>(zero_denominator).is_err());
    }

    #[test]
    fn ottab_deserialization_applies_the_exact_allocation_guardrail() {
        let oversized = json!({
            "kind": "exact",
            "numerator": "1".repeat(DEFAULT_EXACT_LITERAL_LIMITS.max_digits() + 1),
            "denominator": "1"
        });
        let error = serde_json::from_value::<NumericScalar>(oversized).unwrap_err();
        assert!(error.to_string().contains("safe `.ottab` limit"));
    }

    #[test]
    fn exact_arithmetic_remains_exact_and_checked() {
        let one_third = NumericScalar::parse_exact("1/3").unwrap();
        let one_sixth = NumericScalar::parse_exact("1/6").unwrap();
        assert_eq!(
            one_third.checked_add(&one_sixth).unwrap().canonical(),
            "1/2"
        );
        assert_eq!(
            one_third.checked_sub(&one_sixth).unwrap().canonical(),
            "1/6"
        );
        assert_eq!(
            one_third.checked_mul(&one_sixth).unwrap().canonical(),
            "1/18"
        );
        assert_eq!(one_third.checked_div(&one_sixth).unwrap().canonical(), "2");
        let zero = NumericScalar::integer(0);
        assert_eq!(
            one_third.checked_div(&zero).unwrap_err().kind,
            ScalarErrorKind::DivisionByZero
        );
    }

    #[test]
    fn mixed_arithmetic_stays_approximate_and_propagates_a_certificate() {
        let exact = NumericScalar::parse_exact("1/10").unwrap();
        let approximate = NumericScalar::approximate(0.2, certified(0.001)).unwrap();
        let sum = exact.checked_add(&approximate).unwrap();
        let value = sum.approximation().expect("mixed result is approximate");
        assert_eq!(value.value(), 0.30000000000000004);
        assert_eq!(
            value.boundary().method(),
            &ApproximationMethod::PropagatedArithmetic
        );
        assert_eq!(
            value.boundary().propagated_by(),
            Some(ArithmeticOperation::Add)
        );
        assert!(value.boundary().certified_absolute_error().is_some());
    }

    #[test]
    fn conversions_never_relabel_approximation_as_exact() {
        let approximate = NumericScalar::approximate(0.5, certified(0.0)).unwrap();
        assert_eq!(
            approximate.exact_value().unwrap_err().kind,
            ScalarErrorKind::ApproximateCannotBecomeExact
        );
        assert_eq!(
            approximate
                .exact_to_approximate(ApproximationBoundary::binary_f64())
                .unwrap_err()
                .kind,
            ScalarErrorKind::ApproximationRelabeling
        );

        let exact = NumericScalar::parse_exact("0.1").unwrap();
        let converted = exact
            .exact_to_approximate(ApproximationBoundary::binary_f64())
            .unwrap();
        let converted = converted.approximation().unwrap();
        assert!(converted.boundary().certified_absolute_error().is_some());
    }

    #[test]
    fn comparison_uses_certified_intervals_not_centers_alone() {
        let low = NumericScalar::approximate(1.0, certified(0.01)).unwrap();
        let high = NumericScalar::approximate(1.1, certified(0.01)).unwrap();
        assert_eq!(low.compare(&high), ScalarComparison::Less);

        let overlap = NumericScalar::approximate(1.005, certified(0.01)).unwrap();
        assert_eq!(
            low.compare(&overlap),
            ScalarComparison::Indeterminate(ComparisonIndeterminacy::OverlappingBoundaries)
        );

        let uncertified = NumericScalar::approximate(
            4.0,
            ApproximationBoundary::new(ApproximationMethod::Imported).unwrap(),
        )
        .unwrap();
        assert_eq!(
            low.compare(&uncertified),
            ScalarComparison::Indeterminate(ComparisonIndeterminacy::UncertifiedApproximation)
        );
    }

    #[test]
    fn zero_error_can_certify_equality_but_nonzero_overlap_cannot() {
        let exact = NumericScalar::integer(1);
        let exact_binary = NumericScalar::approximate(1.0, certified(0.0)).unwrap();
        assert_eq!(exact.compare(&exact_binary), ScalarComparison::Equal);

        let bounded = NumericScalar::approximate(1.0, certified(f64::EPSILON)).unwrap();
        assert_eq!(
            exact.compare(&bounded),
            ScalarComparison::Indeterminate(ComparisonIndeterminacy::OverlappingBoundaries)
        );
    }

    #[test]
    fn division_refuses_a_certified_denominator_interval_crossing_zero() {
        let numerator = NumericScalar::integer(1);
        let divisor = NumericScalar::approximate(0.01, certified(0.02)).unwrap();
        let error = numerator.checked_div(&divisor).unwrap_err();
        assert_eq!(error.kind, ScalarErrorKind::UnstableDivisionBoundary);
        assert_eq!(error.operation, Some(ArithmeticOperation::Divide));
    }

    #[test]
    fn approximate_construction_rejects_nan_and_infinity() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                NumericScalar::approximate(value, ApproximationBoundary::binary_f64())
                    .unwrap_err()
                    .kind,
                ScalarErrorKind::NonFiniteApproximation
            );
        }
    }

    proptest! {
        #[test]
        fn exact_integer_arithmetic_matches_big_rational(
            left in -1_000_000i64..1_000_000,
            right in -1_000_000i64..1_000_000,
        ) {
            let left_scalar = NumericScalar::integer(left);
            let right_scalar = NumericScalar::integer(right);
            let left_rational = BigRational::from_integer(left.into());
            let right_rational = BigRational::from_integer(right.into());

            prop_assert_eq!(
                left_scalar.checked_add(&right_scalar).unwrap().into_exact().unwrap(),
                &left_rational + &right_rational
            );
            prop_assert_eq!(
                left_scalar.checked_sub(&right_scalar).unwrap().into_exact().unwrap(),
                &left_rational - &right_rational
            );
            prop_assert_eq!(
                left_scalar.checked_mul(&right_scalar).unwrap().into_exact().unwrap(),
                &left_rational * &right_rational
            );
            if right != 0 {
                prop_assert_eq!(
                    left_scalar.checked_div(&right_scalar).unwrap().into_exact().unwrap(),
                    &left_rational / &right_rational
                );
            }
        }

        #[test]
        fn written_scale_is_preserved_exactly(
            coefficient in -1_000_000i64..1_000_000,
            scale in 0u32..9,
        ) {
            let sign = if coefficient < 0 { "-" } else { "" };
            let magnitude = coefficient.unsigned_abs();
            let literal = format!("{sign}{magnitude}e-{scale}");
            let parsed = parse_exact_literal(&literal).unwrap();
            let expected = BigRational::new(
                coefficient.into(),
                BigInt::from(10u8).pow(scale),
            );
            prop_assert_eq!(parsed, expected);
        }

        #[test]
        fn exact_rationals_serde_to_canonical_reduced_form(
            numerator in -1_000_000i64..1_000_000,
            denominator in 1i64..1_000_000,
        ) {
            let value = NumericScalar::rational(numerator, denominator).unwrap();
            let encoded = serde_json::to_string(&value).unwrap();
            let decoded: NumericScalar = serde_json::from_str(&encoded).unwrap();
            prop_assert_eq!(decoded, value);
        }

        #[test]
        fn every_finite_f64_round_trips_as_explicitly_approximate(value in any::<f64>()) {
            prop_assume!(value.is_finite());
            let value = NumericScalar::approximate(value, ApproximationBoundary::binary_f64()).unwrap();
            let encoded = serde_json::to_string(&value).unwrap();
            let decoded: NumericScalar = serde_json::from_str(&encoded).unwrap();
            prop_assert!(decoded.is_approximate());
            prop_assert_eq!(decoded, value);
        }
    }
}
