//! Non-executing semantic analysis for the parsed PhonoScript 3 AST.
//!
//! This pass resolves lexical names, records lightweight static types, checks
//! statically knowable call contracts, and admits literal domain arguments.
//! It never evaluates user code or invokes the phonological engine.
//!
//! The pass is gradual and path-insensitive. Unknown values are deferred to
//! runtime, and project-state obligations such as candidate/constraint lookup,
//! violation-vector width, evaluator-dependent weight admission, and
//! second-order formation remain the checked engine's responsibility.

use std::collections::HashMap;

use num_rational::BigRational;
use num_traits::{Signed, Zero};

use crate::phonoscript_frontend::{
    BinaryOperator, Expression, ExpressionKind, Literal, Program, RelatedSpan, Severity, Span,
    Statement, StatementKind, UnaryOperator,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnalysisDiagnosticCode {
    UndefinedName,
    DuplicateBinding,
    ReturnOutsideFunction,
    ImmutableAssignment,
    ShadowedBinding,
    DuplicateRecordKey,
    InvalidModulePlacement,
    NotCallable,
    Arity,
    TypeMismatch,
    DomainAdmission,
    MissingRecordKey,
    UnsupportedCommand,
}

impl AnalysisDiagnosticCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UndefinedName => "PSA1001",
            Self::DuplicateBinding => "PSA1002",
            Self::ReturnOutsideFunction => "PSA1003",
            Self::ImmutableAssignment => "PSA1004",
            Self::ShadowedBinding => "PSA1005",
            Self::DuplicateRecordKey => "PSA1006",
            Self::InvalidModulePlacement => "PSA1007",
            Self::NotCallable => "PSA1101",
            Self::Arity => "PSA1102",
            Self::TypeMismatch => "PSA1201",
            Self::DomainAdmission => "PSA1202",
            Self::MissingRecordKey => "PSA1203",
            Self::UnsupportedCommand => "PSA1301",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisDiagnostic {
    pub code: AnalysisDiagnosticCode,
    pub severity: Severity,
    pub message: String,
    pub primary: Span,
    pub related: Vec<RelatedSpan>,
    pub help: Option<String>,
}

impl AnalysisDiagnostic {
    fn new(
        code: AnalysisDiagnosticCode,
        severity: Severity,
        message: impl Into<String>,
        primary: Span,
    ) -> Self {
        Self {
            code,
            severity,
            message: message.into(),
            primary,
            related: Vec::new(),
            help: None,
        }
    }

    fn error(code: AnalysisDiagnosticCode, message: impl Into<String>, primary: Span) -> Self {
        Self::new(code, Severity::Error, message, primary)
    }

    fn warning(code: AnalysisDiagnosticCode, message: impl Into<String>, primary: Span) -> Self {
        Self::new(code, Severity::Warning, message, primary)
    }

    fn with_related(mut self, span: Span, message: impl Into<String>) -> Self {
        self.related.push(RelatedSpan {
            span,
            message: message.into(),
        });
        self
    }

    fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DomainType {
    Evaluation,
    LearningResult,
    RankingResult,
    SerialResult,
    SecondOrderResult,
    QCalculusResult,
    MarkData,
    LinearExtensions,
    PhonologicalForm,
    GenerationResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum StaticType {
    Unknown,
    Null,
    Number,
    Boolean,
    Text,
    List(Box<StaticType>),
    Record,
    DomainRecord(DomainType),
    Callable { minimum: usize, maximum: usize },
}

impl StaticType {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Null => "null",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Text => "text",
            Self::List(_) => "list",
            Self::Record => "record",
            Self::DomainRecord(_) => "domain record",
            Self::Callable { .. } => "callable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BindingKind {
    Builtin,
    Import,
    Function,
    Parameter,
    Local,
    Loop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingFact {
    pub id: u32,
    pub name: String,
    pub kind: BindingKind,
    pub mutable: bool,
    pub definition: Span,
    pub scope_depth: usize,
    pub value_type: StaticType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolutionFact {
    pub name: String,
    pub use_span: Span,
    pub binding_id: u32,
    pub definition: Span,
    pub kind: BindingKind,
    pub value_type: StaticType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpressionFact {
    pub span: Span,
    pub value_type: StaticType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallFact {
    pub span: Span,
    pub callee: String,
    pub argument_count: usize,
    pub result_type: StaticType,
    pub statically_admitted: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnalysisFacts {
    pub bindings: Vec<BindingFact>,
    pub resolutions: Vec<ResolutionFact>,
    pub expressions: Vec<ExpressionFact>,
    pub calls: Vec<CallFact>,
}

impl AnalysisFacts {
    pub fn type_at(&self, span: Span) -> Option<&StaticType> {
        self.expressions
            .iter()
            .rev()
            .find(|fact| fact.span == span)
            .map(|fact| &fact.value_type)
    }

    pub fn resolution_at(&self, span: Span) -> Option<&ResolutionFact> {
        self.resolutions.iter().find(|fact| fact.use_span == span)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnalysisReport {
    pub diagnostics: Vec<AnalysisDiagnostic>,
    pub facts: AnalysisFacts,
}

impl AnalysisReport {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }

    pub fn errors(&self) -> impl Iterator<Item = &AnalysisDiagnostic> {
        self.diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

pub fn analyze(program: &Program) -> AnalysisReport {
    Analyzer::new().analyze(program)
}

#[derive(Debug, Clone)]
struct CallableSignature {
    minimum: usize,
    maximum: usize,
    parameters: Vec<ExpectedType>,
    rest: Option<ExpectedType>,
    result: StaticType,
}

#[derive(Debug, Clone)]
struct BindingInfo {
    id: u32,
    name: String,
    kind: BindingKind,
    mutable: bool,
    definition: Span,
    value_type: StaticType,
    signature: Option<CallableSignature>,
}

#[derive(Debug, Default)]
struct Scope {
    bindings: HashMap<String, BindingInfo>,
}

#[derive(Debug, Clone, Copy)]
enum ExpectedType {
    Any,
    Number,
    Boolean,
    Text,
    List,
    Record,
    Collection,
    Selector,
    TextOrList,
    ExactInteger,
    NonNegativeInteger,
    PositiveInteger,
    PositiveNumber,
    NonNegativeNumber,
    ListOfText,
    ListOfNonNegativeIntegers,
}

impl ExpectedType {
    fn display_name(self) -> &'static str {
        match self {
            Self::Any => "any value",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Text => "text",
            Self::List => "list",
            Self::Record => "record",
            Self::Collection => "text, list, or record",
            Self::Selector => "text name or numeric index",
            Self::TextOrList => "text or list",
            Self::ExactInteger => "exact integer",
            Self::NonNegativeInteger => "exact nonnegative integer",
            Self::PositiveInteger => "strictly positive exact integer",
            Self::PositiveNumber => "strictly positive number",
            Self::NonNegativeNumber => "nonnegative number",
            Self::ListOfText => "list of text",
            Self::ListOfNonNegativeIntegers => "list of exact nonnegative integers",
        }
    }
}

struct Analyzer {
    scopes: Vec<Scope>,
    diagnostics: Vec<AnalysisDiagnostic>,
    facts: AnalysisFacts,
    next_binding_id: u32,
    function_depth: usize,
}

impl Analyzer {
    fn new() -> Self {
        let mut analyzer = Self {
            scopes: vec![Scope::default()],
            diagnostics: Vec::new(),
            facts: AnalysisFacts::default(),
            next_binding_id: 1,
            function_depth: 0,
        };
        analyzer.install_builtins();
        analyzer
    }

    fn analyze(mut self, program: &Program) -> AnalysisReport {
        self.analyze_statements(&program.statements);
        AnalysisReport {
            diagnostics: self.diagnostics,
            facts: self.facts,
        }
    }

    fn install_builtins(&mut self) {
        let definition = Span::empty(crate::phonoscript_frontend::SourcePosition::start());
        for name in BUILTIN_NAMES {
            let signature =
                builtin_signature(name).expect("every registered builtin has a signature");
            let info = BindingInfo {
                id: self.allocate_id(),
                name: (*name).to_owned(),
                kind: BindingKind::Builtin,
                mutable: false,
                definition,
                value_type: StaticType::Callable {
                    minimum: signature.minimum,
                    maximum: signature.maximum,
                },
                signature: Some(signature),
            };
            self.scopes[0].bindings.insert((*name).to_owned(), info);
        }
    }

    fn allocate_id(&mut self) -> u32 {
        let id = self.next_binding_id;
        self.next_binding_id = self.next_binding_id.saturating_add(1);
        id
    }

    fn analyze_statements(&mut self, statements: &[Statement]) {
        for statement in statements {
            self.analyze_statement(statement);
        }
    }

    fn analyze_statement(&mut self, statement: &Statement) {
        let top_level = self.scopes.len() == 1;
        match &statement.kind {
            StatementKind::Import {
                import_span,
                bindings,
                ..
            } => {
                if !top_level {
                    self.diagnostics.push(
                        AnalysisDiagnostic::error(
                            AnalysisDiagnosticCode::InvalidModulePlacement,
                            "import declarations are only allowed at module top level",
                            *import_span,
                        )
                        .with_help(
                            "Move this import outside every function, branch, loop, and block.",
                        ),
                    );
                }
                for binding in bindings {
                    self.declare(
                        &binding.local,
                        binding.local_span,
                        BindingKind::Import,
                        false,
                        StaticType::Unknown,
                        None,
                    );
                }
            }
            StatementKind::Binding {
                exported,
                export_span,
                mutable,
                name,
                name_span,
                initializer,
            } => {
                if *exported && !top_level {
                    self.invalid_export_placement(
                        export_span.unwrap_or(statement.span),
                        "exported bindings",
                    );
                }
                let value_type = initializer
                    .as_ref()
                    .map(|value| self.infer_expression(value))
                    .unwrap_or(StaticType::Unknown);
                self.declare(
                    name,
                    *name_span,
                    BindingKind::Local,
                    *mutable,
                    value_type,
                    None,
                );
            }
            StatementKind::Function {
                exported,
                export_span,
                name,
                name_span,
                parameters,
                body,
            } => {
                if *exported && !top_level {
                    self.invalid_export_placement(
                        export_span.unwrap_or(statement.span),
                        "exported functions",
                    );
                }
                let signature = CallableSignature {
                    minimum: parameters.len(),
                    maximum: parameters.len(),
                    parameters: vec![ExpectedType::Any; parameters.len()],
                    rest: None,
                    result: StaticType::Unknown,
                };
                self.declare(
                    name,
                    *name_span,
                    BindingKind::Function,
                    false,
                    StaticType::Callable {
                        minimum: parameters.len(),
                        maximum: parameters.len(),
                    },
                    Some(signature),
                );
                self.push_scope();
                self.function_depth += 1;
                for parameter in parameters {
                    self.declare(
                        &parameter.name,
                        parameter.span,
                        BindingKind::Parameter,
                        false,
                        StaticType::Unknown,
                        None,
                    );
                }
                self.analyze_statements(body);
                self.function_depth -= 1;
                self.pop_scope();
            }
            StatementKind::Block(statements) => {
                self.push_scope();
                self.analyze_statements(statements);
                self.pop_scope();
            }
            StatementKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let ty = self.infer_expression(condition);
                self.expect_expression(condition, &ty, ExpectedType::Boolean, "if condition");
                self.push_scope();
                self.analyze_statements(then_branch);
                self.pop_scope();
                if let Some(branch) = else_branch {
                    self.analyze_statement(branch);
                }
            }
            StatementKind::While { condition, body } => {
                let ty = self.infer_expression(condition);
                self.expect_expression(condition, &ty, ExpectedType::Boolean, "while condition");
                self.push_scope();
                self.analyze_statements(body);
                self.pop_scope();
            }
            StatementKind::For {
                binding,
                binding_span,
                iterable,
                body,
            } => {
                let iterable_type = self.infer_expression(iterable);
                let item_type = match &iterable_type {
                    StaticType::List(item) => (**item).clone(),
                    StaticType::Text => StaticType::Text,
                    StaticType::Unknown => StaticType::Unknown,
                    other => {
                        self.type_diagnostic(
                            iterable.span,
                            "for-loop iterable",
                            "text or list",
                            other,
                        );
                        StaticType::Unknown
                    }
                };
                self.push_scope();
                self.declare(
                    binding,
                    *binding_span,
                    BindingKind::Loop,
                    false,
                    item_type,
                    None,
                );
                self.analyze_statements(body);
                self.pop_scope();
            }
            StatementKind::Return(value) => {
                if self.function_depth == 0 {
                    self.diagnostics.push(
                        AnalysisDiagnostic::error(
                            AnalysisDiagnosticCode::ReturnOutsideFunction,
                            "return may only appear inside a function",
                            statement.span,
                        )
                        .with_help("Move this return into a function body."),
                    );
                }
                if let Some(value) = value {
                    self.infer_expression(value);
                }
            }
            StatementKind::Expression(expression) => {
                self.infer_expression(expression);
            }
            StatementKind::Command(_) => self.diagnostics.push(
                AnalysisDiagnostic::error(
                    AnalysisDiagnosticCode::UnsupportedCommand,
                    "legacy line commands are not admitted by PhonoScript 3",
                    statement.span,
                )
                .with_help("Use a parsed function call such as evaluate()."),
            ),
        }
    }

    fn invalid_export_placement(&mut self, span: Span, declaration: &str) {
        self.diagnostics.push(
            AnalysisDiagnostic::error(
                AnalysisDiagnosticCode::InvalidModulePlacement,
                format!("{declaration} are only allowed at module top level"),
                span,
            )
            .with_help("Move this export outside every function, branch, loop, and block."),
        );
    }

    fn infer_expression(&mut self, expression: &Expression) -> StaticType {
        let value_type = match &expression.kind {
            ExpressionKind::Literal(literal) => match literal {
                Literal::Number(_) => StaticType::Number,
                Literal::Boolean(_) => StaticType::Boolean,
                Literal::Text(_) => StaticType::Text,
                Literal::Null => StaticType::Null,
            },
            ExpressionKind::Variable(name) => self
                .resolve(name, expression.span)
                .map_or(StaticType::Unknown, |(_, binding)| binding.value_type),
            ExpressionKind::List(items) => {
                let mut common: Option<StaticType> = None;
                for item in items {
                    let item_type = self.infer_expression(item);
                    common = Some(match common {
                        None => item_type,
                        Some(ref current) if *current == item_type => item_type,
                        Some(_) => StaticType::Unknown,
                    });
                }
                StaticType::List(Box::new(common.unwrap_or(StaticType::Unknown)))
            }
            ExpressionKind::Record(entries) => {
                let mut first_keys = HashMap::new();
                for entry in entries {
                    if let Some(first_span) = first_keys.insert(entry.key.clone(), entry.key_span) {
                        self.diagnostics.push(
                            AnalysisDiagnostic::error(
                                AnalysisDiagnosticCode::DuplicateRecordKey,
                                format!("duplicate record key {:?}", entry.key),
                                entry.key_span,
                            )
                            .with_related(first_span, "first field with this key is here")
                            .with_help("Give every field in a record a distinct key."),
                        );
                    }
                    self.infer_expression(&entry.value);
                }
                StaticType::Record
            }
            ExpressionKind::Group(inner) => self.infer_expression(inner),
            ExpressionKind::Unary { operator, operand } => {
                let operand_type = self.infer_expression(operand);
                match operator {
                    UnaryOperator::Not => {
                        self.expect_expression(
                            operand,
                            &operand_type,
                            ExpectedType::Boolean,
                            "logical negation",
                        );
                        StaticType::Boolean
                    }
                    UnaryOperator::Positive | UnaryOperator::Negate => {
                        self.expect_expression(
                            operand,
                            &operand_type,
                            ExpectedType::Number,
                            "numeric unary operator",
                        );
                        StaticType::Number
                    }
                }
            }
            ExpressionKind::Binary {
                left,
                operator,
                right,
            } => self.infer_binary(left, *operator, right, expression.span),
            ExpressionKind::Assignment {
                name,
                name_span,
                value,
            } => {
                let value_type = self.infer_expression(value);
                if let Some((scope_index, binding)) = self.resolve(name, *name_span) {
                    if !binding.mutable {
                        self.diagnostics.push(
                            AnalysisDiagnostic::error(
                                AnalysisDiagnosticCode::ImmutableAssignment,
                                format!("binding {name:?} is immutable"),
                                *name_span,
                            )
                            .with_related(binding.definition, "binding declared here")
                            .with_help("Declare the binding with var if reassignment is intended."),
                        );
                    } else if matches!(binding.value_type, StaticType::Unknown | StaticType::Null) {
                        if let Some(existing) = self.scopes[scope_index].bindings.get_mut(name) {
                            existing.value_type = value_type.clone();
                        }
                        if let Some(fact) = self
                            .facts
                            .bindings
                            .iter_mut()
                            .find(|fact| fact.id == binding.id)
                        {
                            fact.value_type = value_type.clone();
                        }
                    }
                }
                value_type
            }
            ExpressionKind::Call { callee, arguments } => {
                self.infer_call(expression.span, callee, arguments)
            }
            ExpressionKind::Index { collection, index } => self.infer_index(collection, index),
            ExpressionKind::Member {
                object,
                field,
                field_span,
            } => self.infer_member(object, field, *field_span),
        };
        self.facts.expressions.push(ExpressionFact {
            span: expression.span,
            value_type: value_type.clone(),
        });
        value_type
    }

    fn infer_index(&mut self, collection: &Expression, index: &Expression) -> StaticType {
        let collection_type = self.infer_expression(collection);
        let index_type = self.infer_expression(index);
        match collection_type {
            StaticType::List(item) => {
                self.expect_expression(
                    index,
                    &index_type,
                    ExpectedType::NonNegativeInteger,
                    "list index",
                );
                *item
            }
            StaticType::Text => {
                self.expect_expression(
                    index,
                    &index_type,
                    ExpectedType::NonNegativeInteger,
                    "text index",
                );
                StaticType::Text
            }
            StaticType::Record | StaticType::DomainRecord(_) => {
                self.expect_expression(index, &index_type, ExpectedType::Text, "record index");
                let Some(key) = constant_text(index) else {
                    return StaticType::Unknown;
                };
                let Some(entries) = constant_record_entries(collection) else {
                    return StaticType::Unknown;
                };
                if let Some(entry) = entries.iter().find(|entry| entry.key == key) {
                    return self
                        .facts
                        .expressions
                        .iter()
                        .rev()
                        .find(|fact| fact.span == entry.value.span)
                        .map_or(StaticType::Unknown, |fact| fact.value_type.clone());
                }
                let mut known = entries
                    .iter()
                    .map(|entry| format!("{:?}", entry.key))
                    .collect::<Vec<_>>();
                known.sort();
                self.diagnostics.push(
                    AnalysisDiagnostic::error(
                        AnalysisDiagnosticCode::MissingRecordKey,
                        format!("record has no field {key:?}"),
                        index.span,
                    )
                    .with_help(if known.is_empty() {
                        "This record has no fields.".to_owned()
                    } else {
                        format!("Available fields: {}.", known.join(", "))
                    }),
                );
                StaticType::Unknown
            }
            StaticType::Unknown => StaticType::Unknown,
            other => {
                self.type_diagnostic(collection.span, "indexing", "text, list, or record", &other);
                StaticType::Unknown
            }
        }
    }

    fn infer_member(&mut self, object: &Expression, field: &str, field_span: Span) -> StaticType {
        let object_type = self.infer_expression(object);
        match object_type {
            StaticType::Record | StaticType::DomainRecord(_) => {
                let Some(entries) = constant_record_entries(object) else {
                    return StaticType::Unknown;
                };
                if let Some(entry) = entries.iter().find(|entry| entry.key == field) {
                    return self
                        .facts
                        .expressions
                        .iter()
                        .rev()
                        .find(|fact| fact.span == entry.value.span)
                        .map_or(StaticType::Unknown, |fact| fact.value_type.clone());
                }
                let mut known = entries
                    .iter()
                    .map(|entry| format!("{:?}", entry.key))
                    .collect::<Vec<_>>();
                known.sort();
                self.diagnostics.push(
                    AnalysisDiagnostic::error(
                        AnalysisDiagnosticCode::MissingRecordKey,
                        format!("record has no field {field:?}"),
                        field_span,
                    )
                    .with_help(if known.is_empty() {
                        "This record has no fields.".to_owned()
                    } else {
                        format!("Available fields: {}.", known.join(", "))
                    }),
                );
                StaticType::Unknown
            }
            StaticType::Unknown => StaticType::Unknown,
            other => {
                self.type_diagnostic(object.span, "member access", "record", &other);
                StaticType::Unknown
            }
        }
    }

    fn infer_binary(
        &mut self,
        left: &Expression,
        operator: BinaryOperator,
        right: &Expression,
        span: Span,
    ) -> StaticType {
        let left_type = self.infer_expression(left);
        let right_type = self.infer_expression(right);
        match operator {
            BinaryOperator::Equal | BinaryOperator::NotEqual => StaticType::Boolean,
            BinaryOperator::And | BinaryOperator::Or => {
                self.expect_expression(left, &left_type, ExpectedType::Boolean, "logical operator");
                self.expect_expression(
                    right,
                    &right_type,
                    ExpectedType::Boolean,
                    "logical operator",
                );
                StaticType::Boolean
            }
            BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
            | BinaryOperator::Remainder => {
                self.expect_expression(left, &left_type, ExpectedType::Number, "numeric operator");
                self.expect_expression(
                    right,
                    &right_type,
                    ExpectedType::Number,
                    "numeric operator",
                );
                StaticType::Number
            }
            BinaryOperator::Add => {
                if matches!(left_type, StaticType::Unknown)
                    || matches!(right_type, StaticType::Unknown)
                {
                    StaticType::Unknown
                } else if left_type == right_type
                    && matches!(
                        left_type,
                        StaticType::Number | StaticType::Text | StaticType::List(_)
                    )
                {
                    left_type
                } else {
                    self.diagnostics.push(AnalysisDiagnostic::error(
                        AnalysisDiagnosticCode::TypeMismatch,
                        format!(
                            "+ requires two numbers, two texts, or two lists; received {} and {}",
                            left_type.display_name(),
                            right_type.display_name()
                        ),
                        span,
                    ));
                    StaticType::Unknown
                }
            }
            BinaryOperator::Less
            | BinaryOperator::LessEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterEqual => {
                if !matches!(left_type, StaticType::Unknown)
                    && !matches!(right_type, StaticType::Unknown)
                    && !((left_type == StaticType::Number && right_type == StaticType::Number)
                        || (left_type == StaticType::Text && right_type == StaticType::Text))
                {
                    self.diagnostics.push(AnalysisDiagnostic::error(
                        AnalysisDiagnosticCode::TypeMismatch,
                        format!(
                            "ordering requires two numbers or two texts; received {} and {}",
                            left_type.display_name(),
                            right_type.display_name()
                        ),
                        span,
                    ));
                }
                StaticType::Boolean
            }
        }
    }

    fn infer_call(
        &mut self,
        call_span: Span,
        callee: &Expression,
        arguments: &[Expression],
    ) -> StaticType {
        let argument_types: Vec<StaticType> = arguments
            .iter()
            .map(|argument| self.infer_expression(argument))
            .collect();
        let diagnostics_before = self.error_count();
        let ExpressionKind::Variable(name) = &callee.kind else {
            self.infer_expression(callee);
            self.diagnostics.push(
                AnalysisDiagnostic::error(
                    AnalysisDiagnosticCode::NotCallable,
                    "PhonoScript calls require a named function or builtin",
                    callee.span,
                )
                .with_help("Bind the function to a name and call that name."),
            );
            return StaticType::Unknown;
        };
        let Some((_, binding)) = self.resolve(name, callee.span) else {
            return StaticType::Unknown;
        };
        self.facts.expressions.push(ExpressionFact {
            span: callee.span,
            value_type: binding.value_type.clone(),
        });
        let Some(signature) = binding.signature else {
            if binding.kind == BindingKind::Import {
                self.facts.calls.push(CallFact {
                    span: call_span,
                    callee: name.clone(),
                    argument_count: arguments.len(),
                    result_type: StaticType::Unknown,
                    statically_admitted: self.error_count() == diagnostics_before,
                });
                return StaticType::Unknown;
            }
            self.diagnostics.push(
                AnalysisDiagnostic::error(
                    AnalysisDiagnosticCode::NotCallable,
                    format!("{name:?} is not callable"),
                    callee.span,
                )
                .with_related(binding.definition, "value declared here"),
            );
            return StaticType::Unknown;
        };
        if !(signature.minimum..=signature.maximum).contains(&arguments.len()) {
            let expected = if signature.minimum == signature.maximum {
                signature.minimum.to_string()
            } else if signature.maximum == usize::MAX {
                format!("at least {}", signature.minimum)
            } else {
                format!("{} through {}", signature.minimum, signature.maximum)
            };
            self.diagnostics.push(AnalysisDiagnostic::error(
                AnalysisDiagnosticCode::Arity,
                format!(
                    "{name} expects {expected} arguments but received {}",
                    arguments.len()
                ),
                call_span,
            ));
        }
        for (index, (argument, actual)) in arguments.iter().zip(&argument_types).enumerate() {
            let expected = signature.parameters.get(index).copied().or(signature.rest);
            if let Some(expected) = expected {
                self.expect_expression(
                    argument,
                    actual,
                    expected,
                    &format!("argument {} of {name}", index + 1),
                );
            }
        }
        if binding.kind == BindingKind::Builtin {
            self.admit_builtin_literals(name, arguments);
        }
        let admitted = self.error_count() == diagnostics_before;
        self.facts.calls.push(CallFact {
            span: call_span,
            callee: name.clone(),
            argument_count: arguments.len(),
            result_type: signature.result.clone(),
            statically_admitted: admitted,
        });
        signature.result
    }

    fn admit_builtin_literals(&mut self, name: &str, arguments: &[Expression]) {
        let allowed: Option<(usize, &[&str], &str)> = match name {
            "project_evaluator" => Some((
                0,
                &[
                    "ot",
                    "optimality_theory",
                    "optimality",
                    "hg",
                    "harmonic_grammar",
                    "harmonicgrammar",
                    "maxent",
                    "maximum_entropy",
                    "maximum_entropy_grammar",
                ],
                "OT, HG, or MaxEnt",
            )),
            "tableau_evaluator" => Some((
                0,
                &[
                    "inherit",
                    "ot",
                    "optimality_theory",
                    "optimality",
                    "hg",
                    "harmonic_grammar",
                    "harmonicgrammar",
                    "maxent",
                    "maximum_entropy",
                    "maximum_entropy_grammar",
                ],
                "inherit, OT, HG, or MaxEnt",
            )),
            "tableau_ties" => Some((
                0,
                &[
                    "retain",
                    "retain_all",
                    "co_winners",
                    "first",
                    "first_listed",
                    "unique",
                    "require_unique",
                ],
                "retain, first, or unique",
            )),
            "serial_side" => Some((0, &["source", "target"], "source or target")),
            "second_query" => Some((
                0,
                &[
                    "winner",
                    "winners",
                    "winner_set",
                    "surface",
                    "surface_winners",
                    "surface_winner_set",
                    "winning_forms",
                    "order",
                    "complete_order",
                    "probability",
                    "probability_law",
                    "support",
                    "candidate_support",
                ],
                "a registered winner, order, probability, or support query",
            )),
            "second_layout" => Some((
                0,
                &[
                    "overlay",
                    "delta",
                    "delta_sidecar",
                    "sidecar",
                    "paired",
                    "expanded",
                    "expanded_paired",
                ],
                "overlay, delta, or paired",
            )),
            "second_mode" => Some((
                0,
                &["exact", "approximate", "approx", "grid", "grid_based"],
                "exact, approximate, or grid",
            )),
            "second_response_domain" => Some((
                0,
                &[
                    "terminal",
                    "terminal_result",
                    "trajectory",
                    "complete_trajectory",
                ],
                "terminal or trajectory",
            )),
            "second_normalizer" => Some((
                0,
                &[
                    "independent",
                    "independent_normalizers",
                    "shared",
                    "shared_declared",
                    "shared_normalizer",
                ],
                "independent or shared",
            )),
            "second_consumer" => Some((
                0,
                &["direct", "later", "later_consumer"],
                "direct or later consumer",
            )),
            "export_tableau" | "export_plot" => {
                Some((1, &["svg", "png", "pdf"], "svg, png, or pdf"))
            }
            _ => None,
        };
        if let Some((index, alternatives, description)) = allowed
            && let Some(argument) = arguments.get(index)
            && let Some(text) = constant_text(argument)
        {
            let normalized = normalize_domain_text(text);
            if !alternatives.contains(&normalized.as_str()) {
                self.domain_diagnostic(
                    argument.span,
                    format!("{name} does not admit {text:?}; expected {description}"),
                );
            }
        }

        if matches!(
            name,
            "project_keyword"
                | "constraint_add"
                | "constraint_add_unweighted"
                | "candidate_add"
                | "tableau_new"
                | "missing_dependency_add"
        ) && let Some(argument) = arguments.first()
            && constant_text(argument).is_some_and(|value| value.trim().is_empty())
        {
            self.domain_diagnostic(
                argument.span,
                format!("argument 1 of {name} cannot be empty"),
            );
        }
        if name == "missing_dependency_add" {
            self.admit_missing_dependency_literals(arguments);
        }
        if name == "range"
            && arguments.len() == 3
            && constant_number(&arguments[2]).is_some_and(|value| value.is_zero())
        {
            self.domain_diagnostic(arguments[2].span, "range step cannot be zero");
        }
    }

    fn admit_missing_dependency_literals(&mut self, arguments: &[Expression]) {
        let literal = |index: usize| arguments.get(index).and_then(constant_text);
        if let Some(stage) = literal(1) {
            let normalized = normalize_domain_text(stage);
            if !["formation", "admission"].contains(&normalized.as_str()) {
                self.domain_diagnostic(
                    arguments[1].span,
                    format!(
                        "missing_dependency_add does not admit stage {stage:?}; expected formation or admission"
                    ),
                );
            }
        }
        let scope = literal(2).map(normalize_domain_text);
        if let Some(scope) = scope.as_deref()
            && ![
                "any_evaluation",
                "evaluator",
                "learning",
                "exact_certification",
            ]
            .contains(&scope)
        {
            self.domain_diagnostic(
                arguments[2].span,
                format!(
                    "missing_dependency_add does not admit scope {:?}; expected any_evaluation, evaluator, learning, or exact_certification",
                    literal(2).unwrap_or_default()
                ),
            );
        }
        if scope.as_deref() == Some("evaluator") && arguments.len() != 7 {
            self.domain_diagnostic(
                arguments[2].span,
                "missing_dependency_add requires argument 7 when scope is evaluator",
            );
        }
        if scope.as_deref().is_some_and(|scope| scope != "evaluator") && arguments.len() == 7 {
            self.domain_diagnostic(
                arguments[6].span,
                "missing_dependency_add accepts an evaluator only when scope is evaluator",
            );
        }
        if let Some(evaluator) = literal(6) {
            let normalized = normalize_domain_text(evaluator);
            if ![
                "ot",
                "optimality_theory",
                "optimality",
                "hg",
                "harmonic_grammar",
                "harmonicgrammar",
                "maxent",
                "maximum_entropy",
                "maximum_entropy_grammar",
            ]
            .contains(&normalized.as_str())
            {
                self.domain_diagnostic(
                    arguments[6].span,
                    format!(
                        "missing_dependency_add does not admit evaluator {evaluator:?}; expected OT, HG, or MaxEnt"
                    ),
                );
            }
        }
        for index in [3, 4, 5] {
            if literal(index).is_some_and(|value| value.trim().is_empty()) {
                self.domain_diagnostic(
                    arguments[index].span,
                    format!(
                        "argument {} of missing_dependency_add cannot be empty",
                        index + 1
                    ),
                );
            }
        }
    }

    fn expect_expression(
        &mut self,
        expression: &Expression,
        actual: &StaticType,
        expected: ExpectedType,
        purpose: &str,
    ) {
        if !type_compatible(actual, expected) {
            self.type_diagnostic(expression.span, purpose, expected.display_name(), actual);
            return;
        }
        let element_expectation = match expected {
            ExpectedType::ListOfText => Some(ExpectedType::Text),
            ExpectedType::ListOfNonNegativeIntegers => Some(ExpectedType::NonNegativeInteger),
            _ => None,
        };
        if let (Some(element_expectation), Some(items)) =
            (element_expectation, constant_list_items(expression))
        {
            for item in items {
                let item_type = self
                    .facts
                    .type_at(item.span)
                    .cloned()
                    .unwrap_or(StaticType::Unknown);
                self.expect_expression(item, &item_type, element_expectation, purpose);
            }
            return;
        }
        let Some(value) = constant_number(expression) else {
            return;
        };
        let admitted = match expected {
            ExpectedType::ExactInteger => value.is_integer(),
            ExpectedType::NonNegativeInteger => value.is_integer() && !value.is_negative(),
            ExpectedType::PositiveInteger => value.is_integer() && value > BigRational::zero(),
            ExpectedType::PositiveNumber => value > BigRational::zero(),
            ExpectedType::NonNegativeNumber => !value.is_negative(),
            _ => true,
        };
        if !admitted {
            self.domain_diagnostic(
                expression.span,
                format!(
                    "{purpose} requires {}; received {value}",
                    expected.display_name()
                ),
            );
        }
    }

    fn type_diagnostic(&mut self, span: Span, purpose: &str, expected: &str, actual: &StaticType) {
        self.diagnostics.push(AnalysisDiagnostic::error(
            AnalysisDiagnosticCode::TypeMismatch,
            format!(
                "{purpose} requires {expected}; received {}",
                actual.display_name()
            ),
            span,
        ));
    }

    fn domain_diagnostic(&mut self, span: Span, message: impl Into<String>) {
        self.diagnostics.push(AnalysisDiagnostic::error(
            AnalysisDiagnosticCode::DomainAdmission,
            message,
            span,
        ));
    }

    fn declare(
        &mut self,
        name: &str,
        definition: Span,
        kind: BindingKind,
        mutable: bool,
        value_type: StaticType,
        signature: Option<CallableSignature>,
    ) {
        let current = self.scopes.len() - 1;
        if let Some(previous) = self.scopes[current].bindings.get(name) {
            self.diagnostics.push(
                AnalysisDiagnostic::error(
                    AnalysisDiagnosticCode::DuplicateBinding,
                    format!("{name:?} is already declared in this lexical scope"),
                    definition,
                )
                .with_related(previous.definition, "previous declaration is here"),
            );
            return;
        }
        if let Some(previous_span) = self.lookup_outer(name).map(|item| item.definition) {
            self.diagnostics.push(
                AnalysisDiagnostic::warning(
                    AnalysisDiagnosticCode::ShadowedBinding,
                    format!("{name:?} shadows an outer binding"),
                    definition,
                )
                .with_related(previous_span, "outer binding is here")
                .with_help("Nested shadowing is legal; rename either binding if it is accidental."),
            );
        }
        let info = BindingInfo {
            id: self.allocate_id(),
            name: name.to_owned(),
            kind,
            mutable,
            definition,
            value_type: value_type.clone(),
            signature,
        };
        self.scopes[current]
            .bindings
            .insert(name.to_owned(), info.clone());
        self.facts.bindings.push(BindingFact {
            id: info.id,
            name: info.name,
            kind,
            mutable,
            definition,
            scope_depth: current,
            value_type,
        });
    }

    fn resolve(&mut self, name: &str, use_span: Span) -> Option<(usize, BindingInfo)> {
        for index in (0..self.scopes.len()).rev() {
            if let Some(binding) = self.scopes[index].bindings.get(name).cloned() {
                self.facts.resolutions.push(ResolutionFact {
                    name: name.to_owned(),
                    use_span,
                    binding_id: binding.id,
                    definition: binding.definition,
                    kind: binding.kind,
                    value_type: binding.value_type.clone(),
                });
                return Some((index, binding));
            }
        }
        self.diagnostics.push(
            AnalysisDiagnostic::error(
                AnalysisDiagnosticCode::UndefinedName,
                format!("undefined name {name:?}"),
                use_span,
            )
            .with_help("Declare the name in this lexical scope before using it."),
        );
        None
    }

    fn lookup_outer(&self, name: &str) -> Option<&BindingInfo> {
        if self.scopes.len() <= 1 {
            return None;
        }
        self.scopes[..self.scopes.len() - 1]
            .iter()
            .rev()
            .find_map(|scope| scope.bindings.get(name))
    }

    fn push_scope(&mut self) {
        self.scopes.push(Scope::default());
    }

    fn pop_scope(&mut self) {
        debug_assert!(self.scopes.len() > 1);
        self.scopes.pop();
    }

    fn error_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Error)
            .count()
    }
}

fn type_compatible(actual: &StaticType, expected: ExpectedType) -> bool {
    if matches!(actual, StaticType::Unknown) || matches!(expected, ExpectedType::Any) {
        return true;
    }
    match expected {
        ExpectedType::Any => true,
        ExpectedType::Number
        | ExpectedType::ExactInteger
        | ExpectedType::NonNegativeInteger
        | ExpectedType::PositiveInteger
        | ExpectedType::PositiveNumber
        | ExpectedType::NonNegativeNumber => matches!(actual, StaticType::Number),
        ExpectedType::Boolean => matches!(actual, StaticType::Boolean),
        ExpectedType::Text => matches!(actual, StaticType::Text),
        ExpectedType::List => matches!(actual, StaticType::List(_)),
        ExpectedType::Record => {
            matches!(actual, StaticType::Record | StaticType::DomainRecord(_))
        }
        ExpectedType::Collection => matches!(
            actual,
            StaticType::Text
                | StaticType::List(_)
                | StaticType::Record
                | StaticType::DomainRecord(_)
        ),
        ExpectedType::Selector => matches!(actual, StaticType::Text | StaticType::Number),
        ExpectedType::TextOrList => matches!(actual, StaticType::Text | StaticType::List(_)),
        ExpectedType::ListOfText => match actual {
            StaticType::List(item) => {
                matches!(item.as_ref(), StaticType::Text | StaticType::Unknown)
            }
            _ => false,
        },
        ExpectedType::ListOfNonNegativeIntegers => match actual {
            StaticType::List(item) => {
                matches!(item.as_ref(), StaticType::Number | StaticType::Unknown)
            }
            _ => false,
        },
    }
}

fn constant_text(expression: &Expression) -> Option<&str> {
    match &expression.kind {
        ExpressionKind::Literal(Literal::Text(value)) => Some(value),
        ExpressionKind::Group(inner) => constant_text(inner),
        _ => None,
    }
}

fn constant_number(expression: &Expression) -> Option<BigRational> {
    match &expression.kind {
        ExpressionKind::Literal(Literal::Number(value)) => Some(value.exact_value()),
        ExpressionKind::Group(inner) => constant_number(inner),
        ExpressionKind::Unary { operator, operand } => {
            let value = constant_number(operand)?;
            match operator {
                UnaryOperator::Positive => Some(value),
                UnaryOperator::Negate => Some(-value),
                UnaryOperator::Not => None,
            }
        }
        _ => None,
    }
}

fn constant_list_items(expression: &Expression) -> Option<&[Expression]> {
    match &expression.kind {
        ExpressionKind::List(items) => Some(items),
        ExpressionKind::Group(inner) => constant_list_items(inner),
        _ => None,
    }
}

fn constant_record_entries(
    expression: &Expression,
) -> Option<&[crate::phonoscript_frontend::RecordEntry]> {
    match &expression.kind {
        ExpressionKind::Record(entries) => Some(entries),
        ExpressionKind::Group(inner) => constant_record_entries(inner),
        ExpressionKind::Member { object, field, .. } => constant_record_entries(object)?
            .iter()
            .find(|entry| entry.key == *field)
            .and_then(|entry| constant_record_entries(&entry.value)),
        ExpressionKind::Index { collection, index } => {
            let field = constant_text(index)?;
            constant_record_entries(collection)?
                .iter()
                .find(|entry| entry.key == field)
                .and_then(|entry| constant_record_entries(&entry.value))
        }
        _ => None,
    }
}

fn normalize_domain_text(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace([' ', '-'], "_")
}

fn sig(
    minimum: usize,
    maximum: usize,
    parameters: &[ExpectedType],
    result: StaticType,
) -> CallableSignature {
    CallableSignature {
        minimum,
        maximum,
        parameters: parameters.to_vec(),
        rest: None,
        result,
    }
}

fn variadic_sig(minimum: usize, rest: ExpectedType, result: StaticType) -> CallableSignature {
    CallableSignature {
        minimum,
        maximum: usize::MAX,
        parameters: Vec::new(),
        rest: Some(rest),
        result,
    }
}

fn domain(value: DomainType) -> StaticType {
    StaticType::DomainRecord(value)
}

fn list(item: StaticType) -> StaticType {
    StaticType::List(Box::new(item))
}

fn builtin_signature(name: &str) -> Option<CallableSignature> {
    use ExpectedType as E;
    use StaticType as T;
    let signature = match name {
        "print" => variadic_sig(0, E::Any, T::Null),
        "assert" => sig(1, 2, &[E::Boolean, E::Any], T::Boolean),
        "assert_equal" => sig(2, 3, &[E::Any, E::Any, E::Any], T::Boolean),
        "assert_approx" => sig(
            3,
            4,
            &[E::Number, E::Number, E::NonNegativeNumber, E::Any],
            T::Boolean,
        ),
        "len" => sig(1, 1, &[E::Collection], T::Number),
        "range" => sig(
            1,
            3,
            &[E::ExactInteger, E::ExactInteger, E::ExactInteger],
            list(T::Number),
        ),
        "type_of" | "to_text" => sig(1, 1, &[E::Any], T::Text),
        "project_restore_v2"
        | "project_title"
        | "project_author"
        | "project_description"
        | "project_keyword"
        | "project_evaluator" => sig(1, 1, &[E::Text], T::Null),
        "project_temperature" => sig(1, 1, &[E::PositiveNumber], T::Null),
        "dataset_clear" | "constraints_clear" | "candidates_clear" | "serial_clear" => {
            sig(0, 0, &[], T::Null)
        }
        "tableau_new" => sig(2, 2, &[E::Text, E::Text], T::Number),
        "tableau_select" => sig(1, 1, &[E::Selector], T::Null),
        "tableau_copy" => sig(2, 2, &[E::Selector, E::Selector], T::Null),
        "tableau_name"
        | "tableau_input"
        | "tableau_notes"
        | "tableau_source_locator"
        | "tableau_evaluator"
        | "tableau_ties" => sig(1, 1, &[E::Text], T::Null),
        "tableau_temperature" => sig(1, 1, &[E::PositiveNumber], T::Null),
        "constraint_add" => sig(
            1,
            4,
            &[E::Text, E::Number, E::Text, E::NonNegativeInteger],
            T::Number,
        ),
        "constraint_add_unweighted" => {
            sig(1, 3, &[E::Text, E::Text, E::NonNegativeInteger], T::Number)
        }
        "missing_dependency_add" => sig(
            6,
            7,
            &[
                E::Text,
                E::Text,
                E::Text,
                E::Text,
                E::Text,
                E::Text,
                E::Text,
            ],
            T::Null,
        ),
        "constraint_remove" => sig(1, 1, &[E::Selector], T::Null),
        "constraint_move" | "constraint_rank" => {
            sig(2, 2, &[E::Selector, E::NonNegativeInteger], T::Null)
        }
        "constraint_tie" => sig(2, 2, &[E::Selector, E::Selector], T::Null),
        "constraint_weight" => sig(2, 2, &[E::Selector, E::Number], T::Null),
        "constraint_definition" => sig(2, 2, &[E::Selector, E::Text], T::Null),
        "constraint_enabled" => sig(2, 2, &[E::Selector, E::Boolean], T::Null),
        "constraint_prior" => sig(3, 3, &[E::Selector, E::Number, E::PositiveNumber], T::Null),
        "candidate_add" => sig(
            3,
            5,
            &[
                E::Text,
                E::Text,
                E::ListOfNonNegativeIntegers,
                E::PositiveNumber,
                E::NonNegativeNumber,
            ],
            T::Number,
        ),
        "candidate_add_structured" => sig(
            3,
            5,
            &[
                E::Text,
                E::Record,
                E::ListOfNonNegativeIntegers,
                E::PositiveNumber,
                E::NonNegativeNumber,
            ],
            T::Number,
        ),
        "candidate_remove" => sig(1, 1, &[E::Selector], T::Null),
        "candidate_move" => sig(2, 2, &[E::Selector, E::NonNegativeInteger], T::Null),
        "candidate_name" | "candidate_form" | "candidate_notes" => {
            sig(2, 2, &[E::Selector, E::Text], T::Null)
        }
        "candidate_mass" => sig(2, 2, &[E::Selector, E::PositiveNumber], T::Null),
        "candidate_observed" => sig(2, 2, &[E::Selector, E::NonNegativeNumber], T::Null),
        "violation_set" => sig(
            3,
            3,
            &[E::Selector, E::Selector, E::NonNegativeInteger],
            T::Null,
        ),
        "violation_get" => sig(2, 2, &[E::Selector, E::Selector], T::Number),
        "evaluate" => sig(0, 0, &[], domain(DomainType::Evaluation)),
        "winners" | "winning_forms" => sig(0, 0, &[], list(T::Text)),
        "harmony" | "probability" => sig(1, 1, &[E::Selector], T::Number),
        "assert_winners" | "assert_winning_forms" => {
            sig(1, 2, &[E::ListOfText, E::Any], T::Boolean)
        }
        "assert_probability" => sig(
            3,
            4,
            &[E::Selector, E::Number, E::NonNegativeNumber, E::Any],
            T::Boolean,
        ),
        "maxent_learn" => sig(
            0,
            1,
            &[E::PositiveInteger],
            domain(DomainType::LearningResult),
        ),
        "infer_ranking" => sig(0, 0, &[], domain(DomainType::RankingResult)),
        "harmonic_bounds" => sig(0, 0, &[], list(T::Record)),
        "unnecessary_constraints" => sig(0, 0, &[], list(T::Text)),
        "serial_side" | "serial_start" => sig(1, 1, &[E::Text], T::Null),
        "serial_limit" => sig(1, 1, &[E::PositiveInteger], T::Null),
        "serial_move" => sig(
            4,
            4,
            &[E::Text, E::Text, E::Text, E::ListOfNonNegativeIntegers],
            T::Null,
        ),
        "serial_evaluate" => sig(0, 0, &[], domain(DomainType::SerialResult)),
        "second_query"
        | "second_answer_sort"
        | "second_scope"
        | "second_transformation"
        | "second_transport"
        | "second_layout"
        | "second_mode"
        | "second_response_domain"
        | "second_normalizer"
        | "second_layer_transport" => sig(1, 1, &[E::Text], T::Null),
        "second_tolerance" => sig(1, 1, &[E::NonNegativeNumber], T::Null),
        "second_grid_step" => sig(1, 1, &[E::PositiveNumber], T::Null),
        "second_layers" | "second_consumer" => sig(2, 2, &[E::Text, E::Text], T::Null),
        "second_compare" => sig(0, 0, &[], domain(DomainType::SecondOrderResult)),
        "q_ranking_space" | "typology" => sig(0, 0, &[], domain(DomainType::QCalculusResult)),
        "q_clone" => sig(1, 1, &[E::Selector], domain(DomainType::QCalculusResult)),
        "mark_data" => sig(1, 1, &[E::Selector], domain(DomainType::MarkData)),
        "constraint_demotion" => sig(1, 1, &[E::Selector], domain(DomainType::RankingResult)),
        "partial_ranking_extensions" => sig(
            2,
            2,
            &[E::List, E::PositiveInteger],
            domain(DomainType::LinearExtensions),
        ),
        "generator_identity" | "generator_delete" | "generator_swap" | "segments" => {
            sig(1, 1, &[E::Text], list(T::Text))
        }
        "generator_insert" | "generator_substitute" => {
            sig(2, 2, &[E::Text, E::TextOrList], list(T::Text))
        }
        "unique" => sig(1, 1, &[E::List], list(T::Unknown)),
        "candidates_from_forms" => sig(2, 2, &[E::ListOfText, E::List], T::Null),
        "phonological_form" => sig(
            2,
            3,
            &[E::Text, E::List, E::Record],
            domain(DomainType::PhonologicalForm),
        ),
        "finite_gen" => sig(
            2,
            2,
            &[E::Record, E::Record],
            domain(DomainType::GenerationResult),
        ),
        "generation_to_tableau" => sig(2, 3, &[E::Record, E::List, E::Boolean], T::Record),
        "save" => sig(1, 1, &[E::Text], T::Text),
        "export_tableau" => sig(2, 3, &[E::Text, E::Text, E::Boolean], T::Text),
        "export_plot" => sig(2, 2, &[E::Text, E::Text], T::Text),
        _ => return None,
    };
    Some(signature)
}

/// Public for runtime/front-end parity checks during integration.
pub const BUILTIN_NAMES: &[&str] = &[
    "print",
    "assert",
    "assert_equal",
    "assert_approx",
    "len",
    "range",
    "type_of",
    "to_text",
    "project_restore_v2",
    "project_title",
    "project_author",
    "project_description",
    "project_keyword",
    "project_evaluator",
    "project_temperature",
    "dataset_clear",
    "tableau_new",
    "tableau_select",
    "tableau_copy",
    "tableau_name",
    "tableau_input",
    "tableau_notes",
    "tableau_source_locator",
    "tableau_evaluator",
    "tableau_temperature",
    "tableau_ties",
    "constraints_clear",
    "constraint_add",
    "constraint_add_unweighted",
    "missing_dependency_add",
    "constraint_remove",
    "constraint_move",
    "constraint_rank",
    "constraint_tie",
    "constraint_weight",
    "constraint_definition",
    "constraint_enabled",
    "constraint_prior",
    "candidates_clear",
    "candidate_add",
    "candidate_add_structured",
    "candidate_remove",
    "candidate_move",
    "candidate_name",
    "candidate_form",
    "candidate_mass",
    "candidate_observed",
    "candidate_notes",
    "violation_set",
    "violation_get",
    "evaluate",
    "winners",
    "winning_forms",
    "harmony",
    "probability",
    "assert_winners",
    "assert_winning_forms",
    "assert_probability",
    "maxent_learn",
    "infer_ranking",
    "harmonic_bounds",
    "unnecessary_constraints",
    "serial_side",
    "serial_start",
    "serial_limit",
    "serial_clear",
    "serial_move",
    "serial_evaluate",
    "second_query",
    "second_answer_sort",
    "second_scope",
    "second_transformation",
    "second_transport",
    "second_layout",
    "second_mode",
    "second_tolerance",
    "second_grid_step",
    "second_response_domain",
    "second_normalizer",
    "second_layers",
    "second_layer_transport",
    "second_consumer",
    "second_compare",
    "q_ranking_space",
    "q_clone",
    "typology",
    "generator_identity",
    "generator_delete",
    "generator_insert",
    "generator_substitute",
    "generator_swap",
    "segments",
    "unique",
    "candidates_from_forms",
    "phonological_form",
    "finite_gen",
    "generation_to_tableau",
    "mark_data",
    "constraint_demotion",
    "partial_ranking_extensions",
    "save",
    "export_tableau",
    "export_plot",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phonoscript_frontend;

    fn report(source: &str) -> AnalysisReport {
        let parsed = phonoscript_frontend::parse(source);
        assert!(
            !parsed.has_errors(),
            "frontend diagnostics: {:?}",
            parsed.diagnostics
        );
        analyze(&parsed.program)
    }

    fn codes(report: &AnalysisReport) -> Vec<&'static str> {
        report
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .collect()
    }

    #[test]
    fn undefined_unicode_name_retains_byte_and_scalar_column_span() {
        let report = report("let λογ = não_definido\n");
        let diagnostic = report
            .diagnostics
            .iter()
            .find(|item| item.code == AnalysisDiagnosticCode::UndefinedName)
            .expect("undefined Unicode name is reported");
        assert_eq!(diagnostic.primary.start.byte, 13);
        assert_eq!(diagnostic.primary.start.line, 1);
        assert_eq!(diagnostic.primary.start.column, 11);
        assert_eq!(diagnostic.primary.end.column, 23);
    }

    #[test]
    fn duplicate_same_scope_is_error_but_nested_shadowing_resolves_inward() {
        let duplicate = report("let x = 1\nlet x = 2\n");
        assert!(codes(&duplicate).contains(&"PSA1002"));

        let shadowed = report("let x = 1\n{ let x = 2\n  print(x)\n}\n");
        assert!(!shadowed.has_errors(), "{:?}", shadowed.diagnostics);
        assert_eq!(codes(&shadowed), ["PSA1005"]);
        let inner = shadowed
            .facts
            .bindings
            .iter()
            .find(|binding| binding.name == "x" && binding.scope_depth == 1)
            .expect("inner binding fact");
        let use_fact = shadowed
            .facts
            .resolutions
            .iter()
            .find(|resolution| resolution.name == "x")
            .expect("inner use resolution");
        assert_eq!(use_fact.binding_id, inner.id);
    }

    #[test]
    fn return_outside_function_and_immutable_assignment_are_static_errors() {
        let report = report("let x = 1\nx = 2\nreturn x\n");
        let observed = codes(&report);
        assert!(observed.contains(&"PSA1003"));
        assert!(observed.contains(&"PSA1004"));
    }

    #[test]
    fn imports_declare_selective_aliases_as_immutable_bindings() {
        let report = report(
            "import { answer as imported_answer, solve } from \"./core.phont\"\nprint(imported_answer)\nsolve(imported_answer)\nimported_answer = 2\n",
        );
        assert_eq!(
            report
                .facts
                .bindings
                .iter()
                .find(|binding| binding.name == "imported_answer")
                .map(|binding| (binding.kind, binding.mutable)),
            Some((BindingKind::Import, false))
        );
        assert!(
            codes(&report).contains(&"PSA1004"),
            "{:?}",
            report.diagnostics
        );
        assert!(
            !report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == AnalysisDiagnosticCode::UndefinedName),
            "{:?}",
            report.diagnostics
        );
        let imported_call = report
            .facts
            .calls
            .iter()
            .find(|call| call.callee == "solve")
            .expect("imported call remains gradual until module linking");
        assert_eq!(imported_call.result_type, StaticType::Unknown);
        assert!(imported_call.statically_admitted);
    }

    #[test]
    fn duplicate_import_aliases_use_the_standard_related_binding_diagnostic() {
        let report =
            report("import { first as selected, second as selected } from \"./core.phont\"\n");
        let duplicate = report
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == AnalysisDiagnosticCode::DuplicateBinding)
            .expect("duplicate local import alias");
        assert_eq!(duplicate.related.len(), 1);
        assert_eq!(duplicate.primary.start.column, 39);
        assert_eq!(duplicate.related[0].span.start.column, 19);
    }

    #[test]
    fn imports_and_exports_are_restricted_to_module_top_level() {
        let report = report(
            r#"export let public = 1
export fn public_fn() { return public }
{
    import { hidden } from "./nested.phont"
    export let nested = hidden
}
fn enclosing() {
    export fn nested_fn() { return 1 }
}
"#,
        );
        let placement: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == AnalysisDiagnosticCode::InvalidModulePlacement)
            .collect();
        assert_eq!(placement.len(), 3, "{:?}", report.diagnostics);
        assert!(placement.iter().all(|diagnostic| diagnostic.help.is_some()));
        assert!(
            !report.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == AnalysisDiagnosticCode::InvalidModulePlacement
                    && diagnostic.primary.start.line <= 2
            }),
            "top-level exports must remain admitted"
        );
    }

    #[test]
    fn record_literals_and_null_have_first_class_static_types() {
        let report = report(
            "let record = {a: 1/3, \"text key\": null, nested: {ok: true}}\nlet field = ({a: 1/3})[\"a\"]\n",
        );
        assert!(!report.has_errors(), "{:?}", report.diagnostics);
        let record = report
            .facts
            .bindings
            .iter()
            .find(|fact| fact.name == "record")
            .expect("record binding fact");
        let field = report
            .facts
            .bindings
            .iter()
            .find(|fact| fact.name == "field")
            .expect("field binding fact");
        assert_eq!(record.value_type, StaticType::Record);
        assert_eq!(field.value_type, StaticType::Number);
        assert!(
            report
                .facts
                .expressions
                .iter()
                .any(|fact| fact.value_type == StaticType::Null)
        );
    }

    #[test]
    fn duplicate_missing_and_mistyped_record_keys_are_distinct_diagnostics() {
        let report = report(
            "let duplicate = {σ: 1, σ: 2}\nlet missing = ({a: 1})[\"b\"]\nlet mistyped = ({a: 1})[0]\n",
        );
        let observed = codes(&report);
        assert!(observed.contains(&"PSA1006"), "{observed:?}");
        assert!(observed.contains(&"PSA1203"), "{observed:?}");
        assert!(observed.contains(&"PSA1201"), "{observed:?}");
        let duplicate = report
            .diagnostics
            .iter()
            .find(|item| item.code == AnalysisDiagnosticCode::DuplicateRecordKey)
            .expect("duplicate key diagnostic");
        assert_eq!(duplicate.related.len(), 1);
        assert_eq!(duplicate.primary.start.column, 24);
    }

    #[test]
    fn member_access_infers_literal_fields_and_reports_precise_missing_names() {
        let report = report(
            "let exact = ({statistics: {retained: 3}}).statistics.retained\nlet missing = ({status: \"complete\"}).reason\n",
        );
        let exact = report
            .facts
            .bindings
            .iter()
            .find(|fact| fact.name == "exact")
            .expect("member result binding");
        assert_eq!(exact.value_type, StaticType::Number);
        let diagnostic = report
            .diagnostics
            .iter()
            .find(|item| item.code == AnalysisDiagnosticCode::MissingRecordKey)
            .expect("missing member diagnostic");
        assert_eq!(diagnostic.primary.start.line, 2);
        assert_eq!(
            diagnostic.primary.end.column - diagnostic.primary.start.column,
            6
        );
    }

    #[test]
    fn structured_phonology_builtins_have_domain_types() {
        let report = report(
            "let form = phonological_form(\"/ab/\", [\"a\", \"b\"])\nlet generated = finite_gen(form, {})\ngeneration_to_tableau(generated, [0], true)\n",
        );
        assert!(!report.has_errors(), "{:?}", report.diagnostics);
        let form = report
            .facts
            .bindings
            .iter()
            .find(|fact| fact.name == "form")
            .expect("form binding");
        let generated = report
            .facts
            .bindings
            .iter()
            .find(|fact| fact.name == "generated")
            .expect("generation binding");
        assert_eq!(
            form.value_type,
            StaticType::DomainRecord(DomainType::PhonologicalForm)
        );
        assert_eq!(
            generated.value_type,
            StaticType::DomainRecord(DomainType::GenerationResult)
        );
    }

    #[test]
    fn generated_candidate_imports_require_phonologist_supplied_marks() {
        let report = report(
            "let forms = generator_delete(\"abc\")\ncandidates_from_forms(forms)\nlet form = phonological_form(\"/ab/\", [\"a\", \"b\"])\nlet generated = finite_gen(form, {})\ngeneration_to_tableau(generated)\n",
        );
        let arity_errors = report
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == AnalysisDiagnosticCode::Arity)
            .count();
        assert_eq!(arity_errors, 2, "{:?}", report.diagnostics);
    }

    #[test]
    fn invalid_domain_call_arity_type_and_admission_are_distinct() {
        let report = report(
            "project_temperature(\"warm\")\nserial_move(\"a\", \"b\", false, [0])\nsecond_compare(1)\nsecond_query(\"unregistered\")\n",
        );
        let observed = codes(&report);
        assert!(observed.contains(&"PSA1201"), "{observed:?}");
        assert!(observed.contains(&"PSA1102"), "{observed:?}");
        assert!(observed.contains(&"PSA1202"), "{observed:?}");
    }

    #[test]
    fn incomplete_ledger_authoring_has_checked_static_signatures() {
        let admitted = report(
            r#"constraint_add_unweighted("C", "published marks only", 0)
missing_dependency_add("MISSING-W", "admission", "evaluator", "constraints.fitted-weights", "weights were not published", "supply verified weights", "MaxEnt")
"#,
        );
        assert!(!admitted.has_errors(), "{:?}", admitted.diagnostics);
        assert_eq!(admitted.facts.calls[0].result_type, StaticType::Number);
        assert_eq!(admitted.facts.calls[1].result_type, StaticType::Null);

        let refused = report(
            r#"constraint_add_unweighted("C", 1)
missing_dependency_add("MISSING-W", "evaluation", "evaluator", "coordinate", "message", "remedy")
missing_dependency_add("MISSING-W", "admission", "learning", "coordinate", "message", "remedy", "MaxEnt")
missing_dependency_add("MISSING-W", "admission", "invented", "coordinate", "message", "remedy")
missing_dependency_add("MISSING-W", "admission", "learning", "coordinate", "message")
"#,
        );
        let observed = codes(&refused);
        assert!(observed.contains(&"PSA1102"), "{observed:?}");
        assert!(observed.contains(&"PSA1201"), "{observed:?}");
        assert!(observed.contains(&"PSA1202"), "{observed:?}");
        assert!(
            refused
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code == AnalysisDiagnosticCode::DomainAdmission)
                .count()
                >= 3,
            "{:?}",
            refused.diagnostics
        );
    }

    #[test]
    fn valid_ot_hg_maxent_serial_second_order_and_q_scripts_are_admitted() {
        let scripts = [
            r#"project_evaluator("OT")
constraints_clear()
candidates_clear()
constraint_add("Faith", 3)
constraint_add("Marked", 1)
candidate_add("faithful", "[faithful]", [0, 2])
candidate_add("repair", "[repair]", [1, 0])
assert_winners(["faithful"])
"#,
            r#"project_evaluator("HG")
constraint_add("C1", 2)
candidate_add("a", "[a]", [0])
evaluate()
"#,
            r#"project_evaluator("MaxEnt")
project_temperature(1)
constraint_add("C1", 2)
candidate_add("a", "[a]", [0], 1, 2)
probability("a")
"#,
            r#"project_evaluator("OT")
constraint_add("C1", 1)
serial_side("source")
serial_start("ab")
serial_clear()
serial_move("ab", "a", "delete", [0])
serial_evaluate()
"#,
            r#"second_query("winner_set")
second_mode("exact")
second_response_domain("terminal")
second_normalizer("independent")
second_compare()
"#,
            r#"project_evaluator("OT")
constraint_add("C1", 1)
candidate_add("a", "[a]", [0])
q_ranking_space()
q_clone("C1")
typology()
"#,
        ];
        for script in scripts {
            let report = report(script);
            assert!(!report.has_errors(), "{script}\n{:?}", report.diagnostics);
        }
    }

    #[test]
    fn calls_are_not_executed_and_domain_result_types_are_recorded() {
        let report = report("let result = second_compare()\nlet q = q_ranking_space()\n");
        assert!(!report.has_errors());
        assert_eq!(report.facts.calls.len(), 2);
        assert_eq!(
            report.facts.calls[0].result_type,
            StaticType::DomainRecord(DomainType::SecondOrderResult)
        );
        assert_eq!(
            report.facts.calls[1].result_type,
            StaticType::DomainRecord(DomainType::QCalculusResult)
        );
        assert!(
            report
                .facts
                .calls
                .iter()
                .all(|call| call.statically_admitted)
        );
    }

    #[test]
    fn every_registered_builtin_has_a_static_signature() {
        for name in BUILTIN_NAMES {
            assert!(
                builtin_signature(name).is_some(),
                "missing signature for {name}"
            );
        }
    }

    #[test]
    fn tableau_source_locator_requires_text_and_returns_null() {
        let valid = report("tableau_source_locator(\"Kager 1999, p. 27\")\n");
        assert!(!valid.has_errors(), "{:?}", valid.diagnostics);
        assert_eq!(valid.facts.calls[0].result_type, StaticType::Null);

        let invalid = report("tableau_source_locator(12)\n");
        assert_eq!(codes(&invalid), vec!["PSA1201"]);
    }

    #[test]
    fn structured_candidate_import_requires_a_record_and_explicit_marks() {
        let valid = report("candidate_add_structured(\"candidate\", {id: 1}, [0, 1])\n");
        assert!(!valid.has_errors(), "{:?}", valid.diagnostics);
        assert_eq!(valid.facts.calls[0].result_type, StaticType::Number);

        let missing_marks = report("candidate_add_structured(\"candidate\", {id: 1})\n");
        assert_eq!(codes(&missing_marks), vec!["PSA1102"]);

        let invalid_structure = report("candidate_add_structured(\"candidate\", [], [0])\n");
        assert_eq!(codes(&invalid_structure), vec!["PSA1201"]);
    }
}
