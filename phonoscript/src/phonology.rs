//! Structured phonological representations and a bounded, auditable `GEN`.
//!
//! The flat tableau model is intentionally retained as an evaluator-facing
//! projection.  This module preserves segments and features, prosody,
//! autosegmental association, morphology, derivational provenance, and
//! separately typed correspondence graphs alongside the phonologist-supplied
//! violation ledger. It never infers or counts violations from that structure.
//! Generation is exhaustive only relative to a declared finite
//! domain.  Resource exhaustion and an exploratory support claim remain
//! visible in the result instead of being mistaken for a complete `GEN`.

// Generation refusals deliberately retain the operation, coordinate, code,
// and remediation context required for auditability. Boxing that public error
// payload merely to reduce stack size would complicate every caller without
// changing the bounded generator's behavior.
#![allow(clippy::result_large_err)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! numeric_id {
    ($name:ident, $inner:ty) => {
        #[derive(
            Debug,
            Clone,
            Copy,
            Default,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Serialize,
            Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub $inner);
    };
}

numeric_id!(CandidateId, u64);
numeric_id!(FormId, u32);
numeric_id!(SegmentId, u32);
numeric_id!(MorphemeId, u32);
numeric_id!(SyllableId, u32);
numeric_id!(MoraId, u32);
numeric_id!(FootId, u32);
numeric_id!(ProsodicWordId, u32);
numeric_id!(TierNodeId, u32);
numeric_id!(CorrespondenceGraphId, u32);
numeric_id!(CorrespondenceLinkId, u32);
numeric_id!(StageId, u32);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FeatureName(pub String);

impl From<&str> for FeatureName {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for FeatureName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum FeatureValue {
    Positive,
    Negative,
    Unspecified,
    Symbol(String),
    Integer(i32),
}

pub type FeatureBundle = BTreeMap<FeatureName, FeatureValue>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum FormRole {
    Underlying,
    Surface,
    Intermediate { stage: StageId },
    Base,
    Reduplicant,
    RelatedSurface { relation: String },
    Sympathetic,
    UserNamed { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum MorphemeKind {
    Root,
    Stem,
    Prefix,
    Suffix,
    Infix,
    Reduplicant,
    Clitic,
    UserNamed { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SegmentOrigin {
    Underlying,
    Inserted,
    Affix { morpheme: String },
    Reduplicated { source: SegmentReference },
    Derived,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SegmentReference {
    pub form: FormId,
    pub segment: SegmentId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentTemplate {
    pub symbol: String,
    #[serde(default)]
    pub features: FeatureBundle,
}

impl SegmentTemplate {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
            features: FeatureBundle::new(),
        }
    }

    pub fn with_feature(mut self, name: impl Into<FeatureName>, value: FeatureValue) -> Self {
        self.features.insert(name.into(), value);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub id: SegmentId,
    pub symbol: String,
    #[serde(default)]
    pub features: FeatureBundle,
    pub origin: SegmentOrigin,
}

impl Segment {
    fn from_template(id: SegmentId, template: &SegmentTemplate, origin: SegmentOrigin) -> Self {
        Self {
            id,
            symbol: template.symbol.clone(),
            features: template.features.clone(),
            origin,
        }
    }
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum StressLevel {
    #[default]
    Unstressed,
    Secondary,
    Primary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Syllable {
    pub id: SyllableId,
    #[serde(default)]
    pub onset: Vec<SegmentId>,
    #[serde(default)]
    pub nucleus: Vec<SegmentId>,
    #[serde(default)]
    pub coda: Vec<SegmentId>,
    #[serde(default)]
    pub stress: StressLevel,
}

impl Syllable {
    pub fn segment_ids(&self) -> impl Iterator<Item = SegmentId> + '_ {
        self.onset
            .iter()
            .chain(&self.nucleus)
            .chain(&self.coda)
            .copied()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mora {
    pub id: MoraId,
    pub syllable: SyllableId,
    #[serde(default)]
    pub bearers: Vec<SegmentId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Foot {
    pub id: FootId,
    pub syllables: Vec<SyllableId>,
    pub head: Option<SyllableId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProsodicWord {
    pub id: ProsodicWordId,
    pub syllables: Vec<SyllableId>,
    #[serde(default)]
    pub morphemes: Vec<MorphemeId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProsodicStructure {
    #[serde(default)]
    pub syllables: Vec<Syllable>,
    #[serde(default)]
    pub moras: Vec<Mora>,
    #[serde(default)]
    pub feet: Vec<Foot>,
    #[serde(default)]
    pub words: Vec<ProsodicWord>,
}

impl ProsodicStructure {
    fn remove_segment(&mut self, segment: SegmentId) {
        for syllable in &mut self.syllables {
            syllable.onset.retain(|item| *item != segment);
            syllable.nucleus.retain(|item| *item != segment);
            syllable.coda.retain(|item| *item != segment);
        }
        let empty_syllables: BTreeSet<_> = self
            .syllables
            .iter()
            .filter(|syllable| syllable.segment_ids().next().is_none())
            .map(|syllable| syllable.id)
            .collect();
        self.syllables
            .retain(|syllable| !empty_syllables.contains(&syllable.id));
        for mora in &mut self.moras {
            mora.bearers.retain(|item| *item != segment);
        }
        self.moras
            .retain(|mora| !empty_syllables.contains(&mora.syllable) && !mora.bearers.is_empty());
        for foot in &mut self.feet {
            foot.syllables
                .retain(|syllable| !empty_syllables.contains(syllable));
            if foot
                .head
                .is_some_and(|head| empty_syllables.contains(&head))
            {
                foot.head = None;
            }
        }
        self.feet.retain(|foot| !foot.syllables.is_empty());
        for word in &mut self.words {
            word.syllables
                .retain(|syllable| !empty_syllables.contains(syllable));
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum ToneValue {
    Level(i8),
    Contour(Vec<i8>),
    Symbol(String),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum TierValue {
    Tone(ToneValue),
    Feature {
        name: FeatureName,
        value: FeatureValue,
    },
    Symbol(String),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TierNode {
    pub id: TierNodeId,
    pub value: TierValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "kebab-case")]
pub enum AssociationTarget {
    Segment(SegmentId),
    Syllable(SyllableId),
    Mora(MoraId),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TierAssociation {
    pub node: TierNodeId,
    pub target: AssociationTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutosegmentalTier {
    pub name: String,
    pub nodes: Vec<TierNode>,
    #[serde(default)]
    pub associations: Vec<TierAssociation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Morpheme {
    pub id: MorphemeId,
    pub label: String,
    pub kind: MorphemeKind,
    pub segments: Vec<SegmentId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MorphemeTemplate {
    pub label: String,
    pub kind: MorphemeKind,
    pub segments: Vec<SegmentTemplate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhonologicalForm {
    pub id: FormId,
    pub label: String,
    pub role: FormRole,
    pub segments: Vec<Segment>,
    #[serde(default)]
    pub morphemes: Vec<Morpheme>,
    #[serde(default)]
    pub prosody: ProsodicStructure,
    #[serde(default)]
    pub tiers: Vec<AutosegmentalTier>,
}

impl PhonologicalForm {
    pub fn display_string(&self) -> String {
        self.segments
            .iter()
            .map(|segment| segment.symbol.as_str())
            .collect()
    }

    fn next_segment_id(&self) -> Option<SegmentId> {
        self.segments
            .iter()
            .map(|segment| segment.id.0)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .map(SegmentId)
    }

    fn next_morpheme_id(&self) -> Option<MorphemeId> {
        self.morphemes
            .iter()
            .map(|morpheme| morpheme.id.0)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .map(MorphemeId)
    }

    pub fn validate(&self) -> Vec<StructureIssue> {
        let mut issues = Vec::new();
        let segment_ids: BTreeSet<_> = self.segments.iter().map(|item| item.id).collect();
        if segment_ids.len() != self.segments.len() {
            issues.push(StructureIssue::new(
                StructureIssueCode::DuplicateId,
                format!("form[{}].segments", self.id.0),
                "segment identifiers must be unique within a form",
            ));
        }
        let morpheme_ids: BTreeSet<_> = self.morphemes.iter().map(|item| item.id).collect();
        if morpheme_ids.len() != self.morphemes.len() {
            issues.push(StructureIssue::new(
                StructureIssueCode::DuplicateId,
                format!("form[{}].morphemes", self.id.0),
                "morpheme identifiers must be unique within a form",
            ));
        }
        for morpheme in &self.morphemes {
            for segment in &morpheme.segments {
                if !segment_ids.contains(segment) {
                    issues.push(StructureIssue::new(
                        StructureIssueCode::DanglingReference,
                        format!("form[{}].morpheme[{}]", self.id.0, morpheme.id.0),
                        format!("unknown segment {}", segment.0),
                    ));
                }
            }
        }

        let syllable_ids: BTreeSet<_> = self.prosody.syllables.iter().map(|item| item.id).collect();
        if syllable_ids.len() != self.prosody.syllables.len() {
            issues.push(StructureIssue::new(
                StructureIssueCode::DuplicateId,
                format!("form[{}].prosody.syllables", self.id.0),
                "syllable identifiers must be unique",
            ));
        }
        let mut parsed_segments = BTreeSet::new();
        for syllable in &self.prosody.syllables {
            if syllable.nucleus.is_empty() {
                issues.push(StructureIssue::new(
                    StructureIssueCode::InvalidProsody,
                    format!("form[{}].syllable[{}].nucleus", self.id.0, syllable.id.0),
                    "a syllable must have a declared nucleus",
                ));
            }
            for segment in syllable.segment_ids() {
                if !segment_ids.contains(&segment) {
                    issues.push(StructureIssue::new(
                        StructureIssueCode::DanglingReference,
                        format!("form[{}].syllable[{}]", self.id.0, syllable.id.0),
                        format!("unknown segment {}", segment.0),
                    ));
                }
                if !parsed_segments.insert(segment) {
                    issues.push(StructureIssue::new(
                        StructureIssueCode::InvalidProsody,
                        format!("form[{}].syllable[{}]", self.id.0, syllable.id.0),
                        format!(
                            "segment {} belongs to more than one syllabic position",
                            segment.0
                        ),
                    ));
                }
            }
        }
        let mora_ids: BTreeSet<_> = self.prosody.moras.iter().map(|item| item.id).collect();
        if mora_ids.len() != self.prosody.moras.len() {
            issues.push(StructureIssue::new(
                StructureIssueCode::DuplicateId,
                format!("form[{}].prosody.moras", self.id.0),
                "mora identifiers must be unique",
            ));
        }
        for mora in &self.prosody.moras {
            if !syllable_ids.contains(&mora.syllable) {
                issues.push(StructureIssue::new(
                    StructureIssueCode::DanglingReference,
                    format!("form[{}].mora[{}]", self.id.0, mora.id.0),
                    format!("unknown syllable {}", mora.syllable.0),
                ));
            }
            for segment in &mora.bearers {
                if !segment_ids.contains(segment) {
                    issues.push(StructureIssue::new(
                        StructureIssueCode::DanglingReference,
                        format!("form[{}].mora[{}]", self.id.0, mora.id.0),
                        format!("unknown segment {}", segment.0),
                    ));
                }
            }
        }
        let foot_ids: BTreeSet<_> = self.prosody.feet.iter().map(|item| item.id).collect();
        if foot_ids.len() != self.prosody.feet.len() {
            issues.push(StructureIssue::new(
                StructureIssueCode::DuplicateId,
                format!("form[{}].prosody.feet", self.id.0),
                "foot identifiers must be unique",
            ));
        }
        for foot in &self.prosody.feet {
            for syllable in &foot.syllables {
                if !syllable_ids.contains(syllable) {
                    issues.push(StructureIssue::new(
                        StructureIssueCode::DanglingReference,
                        format!("form[{}].foot[{}]", self.id.0, foot.id.0),
                        format!("unknown syllable {}", syllable.0),
                    ));
                }
            }
            if foot
                .head
                .is_some_and(|head| !foot.syllables.contains(&head))
            {
                issues.push(StructureIssue::new(
                    StructureIssueCode::InvalidProsody,
                    format!("form[{}].foot[{}].head", self.id.0, foot.id.0),
                    "the foot head must be a member of that foot",
                ));
            }
        }
        for word in &self.prosody.words {
            for syllable in &word.syllables {
                if !syllable_ids.contains(syllable) {
                    issues.push(StructureIssue::new(
                        StructureIssueCode::DanglingReference,
                        format!("form[{}].prosodic-word[{}]", self.id.0, word.id.0),
                        format!("unknown syllable {}", syllable.0),
                    ));
                }
            }
            for morpheme in &word.morphemes {
                if !morpheme_ids.contains(morpheme) {
                    issues.push(StructureIssue::new(
                        StructureIssueCode::DanglingReference,
                        format!("form[{}].prosodic-word[{}]", self.id.0, word.id.0),
                        format!("unknown morpheme {}", morpheme.0),
                    ));
                }
            }
        }

        let word_ids: BTreeSet<_> = self.prosody.words.iter().map(|item| item.id).collect();
        if word_ids.len() != self.prosody.words.len() {
            issues.push(StructureIssue::new(
                StructureIssueCode::DuplicateId,
                format!("form[{}].prosody.words", self.id.0),
                "prosodic-word identifiers must be unique",
            ));
        }

        let mut tier_names = BTreeSet::new();
        for tier in &self.tiers {
            if !tier_names.insert(tier.name.as_str()) {
                issues.push(StructureIssue::new(
                    StructureIssueCode::DuplicateId,
                    format!("form[{}].tiers", self.id.0),
                    format!("tier name {:?} is duplicated", tier.name),
                ));
            }
            let node_ids: BTreeSet<_> = tier.nodes.iter().map(|node| node.id).collect();
            if node_ids.len() != tier.nodes.len() {
                issues.push(StructureIssue::new(
                    StructureIssueCode::DuplicateId,
                    format!("form[{}].tier[{}]", self.id.0, tier.name),
                    "tier node identifiers must be unique within a tier",
                ));
            }
            for association in &tier.associations {
                if !node_ids.contains(&association.node) {
                    issues.push(StructureIssue::new(
                        StructureIssueCode::DanglingReference,
                        format!("form[{}].tier[{}]", self.id.0, tier.name),
                        format!("unknown tier node {}", association.node.0),
                    ));
                }
                let valid_target = match association.target {
                    AssociationTarget::Segment(id) => segment_ids.contains(&id),
                    AssociationTarget::Syllable(id) => syllable_ids.contains(&id),
                    AssociationTarget::Mora(id) => mora_ids.contains(&id),
                };
                if !valid_target {
                    issues.push(StructureIssue::new(
                        StructureIssueCode::DanglingReference,
                        format!("form[{}].tier[{}]", self.id.0, tier.name),
                        "association target is not present in this form",
                    ));
                }
            }
        }
        issues
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UnderlyingForm(pub PhonologicalForm);

impl UnderlyingForm {
    pub fn from_segments(
        label: impl Into<String>,
        segments: impl IntoIterator<Item = SegmentTemplate>,
    ) -> Self {
        let label = label.into();
        let segments = segments
            .into_iter()
            .enumerate()
            .map(|(index, template)| {
                Segment::from_template(
                    SegmentId(index as u32),
                    &template,
                    SegmentOrigin::Underlying,
                )
            })
            .collect();
        Self(PhonologicalForm {
            id: FormId(0),
            label,
            role: FormRole::Underlying,
            segments,
            morphemes: Vec::new(),
            prosody: ProsodicStructure::default(),
            tiers: Vec::new(),
        })
    }

    pub fn try_new(form: PhonologicalForm) -> Result<Self, StructureError> {
        let mut issues = form.validate();
        if form.role != FormRole::Underlying {
            issues.push(StructureIssue::new(
                StructureIssueCode::WrongFormRole,
                "underlying.role",
                "an UnderlyingForm must have the underlying role",
            ));
        }
        if issues.is_empty() {
            Ok(Self(form))
        } else {
            Err(StructureError { issues })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SurfaceForm(pub PhonologicalForm);

impl SurfaceForm {
    pub fn try_new(form: PhonologicalForm) -> Result<Self, StructureError> {
        let mut issues = form.validate();
        if form.role != FormRole::Surface {
            issues.push(StructureIssue::new(
                StructureIssueCode::WrongFormRole,
                "surface.role",
                "a SurfaceForm must have the surface role",
            ));
        }
        if issues.is_empty() {
            Ok(Self(form))
        } else {
            Err(StructureError { issues })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "name", rename_all = "kebab-case")]
pub enum CorrespondenceKind {
    InputOutput,
    BaseReduplicant,
    OutputOutput,
    Sympathy,
    UserNamed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "kebab-case")]
pub enum CorrespondenceNode {
    Segment(SegmentId),
    Morpheme(MorphemeId),
    Syllable(SyllableId),
    Mora(MoraId),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorrespondenceLink {
    pub id: CorrespondenceLinkId,
    /// Empty only for insertion relative to this correspondence relation.
    #[serde(default)]
    pub source: Vec<CorrespondenceNode>,
    /// Empty only for deletion relative to this correspondence relation.
    #[serde(default)]
    pub target: Vec<CorrespondenceNode>,
}

impl CorrespondenceLink {
    pub fn pair(
        id: CorrespondenceLinkId,
        source: CorrespondenceNode,
        target: CorrespondenceNode,
    ) -> Self {
        Self {
            id,
            source: vec![source],
            target: vec![target],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorrespondenceGraph {
    pub id: CorrespondenceGraphId,
    pub label: String,
    pub kind: CorrespondenceKind,
    pub source_form: FormId,
    pub target_form: FormId,
    pub links: Vec<CorrespondenceLink>,
}

impl CorrespondenceGraph {
    pub fn identity_segments(
        id: CorrespondenceGraphId,
        label: impl Into<String>,
        kind: CorrespondenceKind,
        source: &PhonologicalForm,
        target: &PhonologicalForm,
    ) -> Self {
        let links = source
            .segments
            .iter()
            .zip(&target.segments)
            .enumerate()
            .map(|(index, (source, target))| {
                CorrespondenceLink::pair(
                    CorrespondenceLinkId(index as u32),
                    CorrespondenceNode::Segment(source.id),
                    CorrespondenceNode::Segment(target.id),
                )
            })
            .collect();
        Self {
            id,
            label: label.into(),
            kind,
            source_form: source.id,
            target_form: target.id,
            links,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DerivationStep {
    pub operation_id: String,
    pub operation_class: OperationClass,
    pub edits: Vec<StructuralEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum StructuralEdit {
    Identity,
    Delete {
        segment: SegmentId,
        position: usize,
    },
    Insert {
        segment: SegmentId,
        position: usize,
    },
    FeatureChange {
        segment: SegmentId,
        feature: FeatureName,
        from: Option<FeatureValue>,
        to: FeatureValue,
    },
    Metathesis {
        left: SegmentId,
        right: SegmentId,
    },
    Affix {
        morpheme: MorphemeId,
        position: usize,
    },
    Reduplicate {
        source: Vec<SegmentId>,
        copies: Vec<SegmentId>,
    },
    Syllabify {
        syllables: usize,
    },
    AssignStress {
        primary: Option<SyllableId>,
    },
    AssignTone {
        tier: String,
        nodes: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuredCandidate {
    pub id: CandidateId,
    /// Presentation label. It is not used as the structural deduplication key.
    pub label: String,
    pub underlying_form: FormId,
    pub surface_form: FormId,
    pub forms: BTreeMap<FormId, PhonologicalForm>,
    #[serde(default)]
    pub correspondences: Vec<CorrespondenceGraph>,
    #[serde(default)]
    pub derivation: Vec<DerivationStep>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

impl StructuredCandidate {
    pub fn identity(input: &UnderlyingForm) -> Self {
        let mut underlying = input.0.clone();
        underlying.id = FormId(0);
        underlying.role = FormRole::Underlying;
        let mut surface = underlying.clone();
        surface.id = FormId(1);
        surface.role = FormRole::Surface;
        surface.label = surface.display_string();
        let io = CorrespondenceGraph::identity_segments(
            CorrespondenceGraphId(0),
            "IO",
            CorrespondenceKind::InputOutput,
            &underlying,
            &surface,
        );
        let label = surface.display_string();
        let forms = [(underlying.id, underlying), (surface.id, surface)]
            .into_iter()
            .collect();
        Self {
            id: CandidateId(0),
            label,
            underlying_form: FormId(0),
            surface_form: FormId(1),
            forms,
            correspondences: vec![io],
            derivation: Vec::new(),
            metadata: BTreeMap::new(),
        }
    }

    pub fn underlying(&self) -> &PhonologicalForm {
        &self.forms[&self.underlying_form]
    }

    pub fn surface(&self) -> &PhonologicalForm {
        &self.forms[&self.surface_form]
    }

    pub fn surface_mut(&mut self) -> &mut PhonologicalForm {
        self.forms
            .get_mut(&self.surface_form)
            .expect("StructuredCandidate invariant: surface form is present")
    }

    pub fn surface_string(&self) -> String {
        self.surface().display_string()
    }

    pub fn correspondence(&self, kind: &CorrespondenceKind) -> Option<&CorrespondenceGraph> {
        self.correspondences
            .iter()
            .find(|graph| &graph.kind == kind)
    }

    pub fn correspondences_of_kind(
        &self,
        kind: CorrespondenceKind,
    ) -> impl Iterator<Item = &CorrespondenceGraph> {
        self.correspondences
            .iter()
            .filter(move |graph| graph.kind == kind)
    }

    pub fn add_related_form(
        &mut self,
        mut form: PhonologicalForm,
    ) -> Result<FormId, StructureError> {
        let next = self
            .forms
            .keys()
            .map(|id| id.0)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| {
                StructureError::single(
                    StructureIssueCode::IdSpaceExhausted,
                    "candidate.forms",
                    "form identifier space exhausted",
                )
            })?;
        form.id = FormId(next);
        let issues = form.validate();
        if !issues.is_empty() {
            return Err(StructureError { issues });
        }
        self.forms.insert(form.id, form);
        Ok(FormId(next))
    }

    pub fn add_correspondence(
        &mut self,
        mut graph: CorrespondenceGraph,
    ) -> Result<CorrespondenceGraphId, StructureError> {
        let next = self
            .correspondences
            .iter()
            .map(|item| item.id.0)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| {
                StructureError::single(
                    StructureIssueCode::IdSpaceExhausted,
                    "candidate.correspondences",
                    "correspondence graph identifier space exhausted",
                )
            })?;
        graph.id = CorrespondenceGraphId(next);
        self.correspondences.push(graph);
        let issues = self.validate();
        if issues.is_empty() {
            Ok(CorrespondenceGraphId(next))
        } else {
            self.correspondences.pop();
            Err(StructureError { issues })
        }
    }

    pub fn validate(&self) -> Vec<StructureIssue> {
        let mut issues = Vec::new();
        if !self.forms.contains_key(&self.underlying_form) {
            issues.push(StructureIssue::new(
                StructureIssueCode::MissingPrimaryForm,
                "candidate.underlying-form",
                "underlying form is absent",
            ));
        }
        if !self.forms.contains_key(&self.surface_form) {
            issues.push(StructureIssue::new(
                StructureIssueCode::MissingPrimaryForm,
                "candidate.surface-form",
                "surface form is absent",
            ));
        }
        if self
            .forms
            .get(&self.underlying_form)
            .is_some_and(|form| form.role != FormRole::Underlying)
        {
            issues.push(StructureIssue::new(
                StructureIssueCode::WrongFormRole,
                "candidate.underlying-form.role",
                "the primary underlying form must carry the underlying role",
            ));
        }
        if self
            .forms
            .get(&self.surface_form)
            .is_some_and(|form| form.role != FormRole::Surface)
        {
            issues.push(StructureIssue::new(
                StructureIssueCode::WrongFormRole,
                "candidate.surface-form.role",
                "the primary surface form must carry the surface role",
            ));
        }
        for (id, form) in &self.forms {
            if id != &form.id {
                issues.push(StructureIssue::new(
                    StructureIssueCode::MismatchedId,
                    format!("candidate.forms[{}]", id.0),
                    format!(
                        "map key {} differs from embedded form id {}",
                        id.0, form.id.0
                    ),
                ));
            }
            issues.extend(form.validate());
        }
        let graph_ids: BTreeSet<_> = self.correspondences.iter().map(|item| item.id).collect();
        if graph_ids.len() != self.correspondences.len() {
            issues.push(StructureIssue::new(
                StructureIssueCode::DuplicateId,
                "candidate.correspondences",
                "correspondence graph identifiers must be unique",
            ));
        }
        for graph in &self.correspondences {
            let Some(source) = self.forms.get(&graph.source_form) else {
                issues.push(StructureIssue::new(
                    StructureIssueCode::DanglingReference,
                    format!("candidate.correspondence[{}].source", graph.id.0),
                    "source form is absent",
                ));
                continue;
            };
            let Some(target) = self.forms.get(&graph.target_form) else {
                issues.push(StructureIssue::new(
                    StructureIssueCode::DanglingReference,
                    format!("candidate.correspondence[{}].target", graph.id.0),
                    "target form is absent",
                ));
                continue;
            };
            let roles_are_compatible = match &graph.kind {
                CorrespondenceKind::InputOutput => {
                    source.role == FormRole::Underlying && is_surface_family_role(&target.role)
                }
                CorrespondenceKind::BaseReduplicant => {
                    source.role == FormRole::Base && target.role == FormRole::Reduplicant
                }
                CorrespondenceKind::OutputOutput => {
                    is_surface_family_role(&source.role) && is_surface_family_role(&target.role)
                }
                CorrespondenceKind::Sympathy => {
                    source.role == FormRole::Sympathetic || target.role == FormRole::Sympathetic
                }
                CorrespondenceKind::UserNamed(_) => true,
            };
            if !roles_are_compatible {
                issues.push(StructureIssue::new(
                    StructureIssueCode::WrongFormRole,
                    format!("candidate.correspondence[{}].kind", graph.id.0),
                    "correspondence endpoints do not match the declared IO, BR, OO, or Sympathy family",
                ));
            }
            let link_ids: BTreeSet<_> = graph.links.iter().map(|item| item.id).collect();
            if link_ids.len() != graph.links.len() {
                issues.push(StructureIssue::new(
                    StructureIssueCode::DuplicateId,
                    format!("candidate.correspondence[{}].links", graph.id.0),
                    "correspondence link identifiers must be unique within a graph",
                ));
            }
            for link in &graph.links {
                if link.source.is_empty() && link.target.is_empty() {
                    issues.push(StructureIssue::new(
                        StructureIssueCode::EmptyCorrespondenceLink,
                        format!(
                            "candidate.correspondence[{}].link[{}]",
                            graph.id.0, link.id.0
                        ),
                        "a correspondence link cannot be empty on both sides",
                    ));
                }
                for node in &link.source {
                    if !form_contains_node(source, node) {
                        issues.push(StructureIssue::new(
                            StructureIssueCode::DanglingReference,
                            format!(
                                "candidate.correspondence[{}].link[{}].source",
                                graph.id.0, link.id.0
                            ),
                            "source correspondent is absent from the source form",
                        ));
                    }
                }
                for node in &link.target {
                    if !form_contains_node(target, node) {
                        issues.push(StructureIssue::new(
                            StructureIssueCode::DanglingReference,
                            format!(
                                "candidate.correspondence[{}].link[{}].target",
                                graph.id.0, link.id.0
                            ),
                            "target correspondent is absent from the target form",
                        ));
                    }
                }
            }
        }
        issues
    }

    pub fn to_flat_candidate(
        &self,
        violations: Vec<u16>,
    ) -> Result<crate::model::Candidate, StructureError> {
        let mut candidate = self.to_flat_candidate_with(violations, 1.0, 0.0, String::new())?;
        candidate.base_mass = crate::exact::NumericScalar::integer(1);
        candidate.observed_frequency = crate::exact::NumericScalar::integer(0);
        Ok(candidate)
    }

    pub fn to_flat_candidate_with(
        &self,
        violations: Vec<u16>,
        base_mass: f64,
        observed_frequency: f64,
        notes: String,
    ) -> Result<crate::model::Candidate, StructureError> {
        let issues = self.validate();
        if !issues.is_empty() {
            return Err(StructureError { issues });
        }
        if !base_mass.is_finite() || base_mass <= 0.0 {
            return Err(StructureError::single(
                StructureIssueCode::InvalidNumericValue,
                "candidate.base-mass",
                "base mass must be finite and strictly positive",
            ));
        }
        if !observed_frequency.is_finite() || observed_frequency < 0.0 {
            return Err(StructureError::single(
                StructureIssueCode::InvalidNumericValue,
                "candidate.observed-frequency",
                "observed frequency must be finite and nonnegative",
            ));
        }
        Ok(crate::model::Candidate {
            id: format!("structured-candidate-{}", self.id.0),
            name: if self.label.trim().is_empty() {
                self.surface_string()
            } else {
                self.label.clone()
            },
            form: self.surface_string(),
            violations,
            base_mass: crate::exact::NumericScalar::approximate(
                base_mass,
                crate::exact::ApproximationBoundary::binary_f64(),
            )
            .expect("validated finite base mass"),
            notes,
            observed_frequency: crate::exact::NumericScalar::approximate(
                observed_frequency,
                crate::exact::ApproximationBoundary::binary_f64(),
            )
            .expect("validated finite observation"),
            structured: Some(self.clone()),
        })
    }

    fn next_graph_id(&self) -> Option<CorrespondenceGraphId> {
        self.correspondences
            .iter()
            .map(|graph| graph.id.0)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .map(CorrespondenceGraphId)
    }

    fn io_mut(&mut self) -> Option<&mut CorrespondenceGraph> {
        let underlying = self.underlying_form;
        let surface = self.surface_form;
        self.correspondences.iter_mut().find(|graph| {
            graph.kind == CorrespondenceKind::InputOutput
                && graph.source_form == underlying
                && graph.target_form == surface
        })
    }

    fn remove_surface_segment_from_correspondence(&mut self, segment: SegmentId) {
        if let Some(io) = self.io_mut() {
            for link in &mut io.links {
                link.target
                    .retain(|node| node != &CorrespondenceNode::Segment(segment));
            }
            io.links
                .retain(|link| !link.source.is_empty() || !link.target.is_empty());
        }
    }

    fn register_surface_insertion(&mut self, segment: SegmentId) -> Result<(), StructureError> {
        let io = self.io_mut().ok_or_else(|| {
            StructureError::single(
                StructureIssueCode::MissingCorrespondence,
                "candidate.correspondences.io",
                "surface insertion requires an input-output correspondence graph",
            )
        })?;
        let next = io
            .links
            .iter()
            .map(|link| link.id.0)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| {
                StructureError::single(
                    StructureIssueCode::IdSpaceExhausted,
                    "candidate.correspondences.io.links",
                    "correspondence link identifier space exhausted",
                )
            })?;
        io.links.push(CorrespondenceLink {
            id: CorrespondenceLinkId(next),
            source: Vec::new(),
            target: vec![CorrespondenceNode::Segment(segment)],
        });
        Ok(())
    }

    fn canonical_key(&self, policy: DeduplicationPolicy) -> Vec<u8> {
        match policy {
            DeduplicationPolicy::SurfaceForm => {
                let normalized = self.canonicalized(false);
                serde_json::to_vec(normalized.surface())
                    .unwrap_or_else(|_| self.surface_string().into_bytes())
            }
            DeduplicationPolicy::StructuredRepresentation
            | DeduplicationPolicy::PreserveDerivations => {
                let clone = self.canonicalized(policy == DeduplicationPolicy::PreserveDerivations);
                serde_json::to_vec(&clone).unwrap_or_else(|_| self.surface_string().into_bytes())
            }
        }
    }

    /// Renumber presentation-local identifiers by structural order. Stable
    /// project identifiers remain in the stored candidate; this normalized
    /// copy exists only for equality, deduplication, and deterministic hashes.
    fn canonicalized(&self, preserve_derivation: bool) -> Self {
        let mut ordered_forms: Vec<_> = self.forms.values().cloned().collect();
        ordered_forms.sort_by(|left, right| {
            canonical_form_rank(&left.role)
                .cmp(&canonical_form_rank(&right.role))
                .then_with(|| left.role.cmp(&right.role))
                .then_with(|| left.display_string().cmp(&right.display_string()))
                .then_with(|| left.id.cmp(&right.id))
        });
        // The declared primary forms are always first in their role. This
        // matters if a user stores an additional form with the same role.
        ordered_forms.sort_by_key(|form| {
            if form.id == self.underlying_form {
                0_u8
            } else if form.id == self.surface_form {
                1
            } else {
                2
            }
        });

        let form_map: BTreeMap<_, _> = ordered_forms
            .iter()
            .enumerate()
            .map(|(index, form)| (form.id, FormId(index as u32)))
            .collect();
        let local_maps: BTreeMap<_, _> = ordered_forms
            .iter()
            .map(|form| {
                let maps = CanonicalLocalMaps {
                    segments: form
                        .segments
                        .iter()
                        .enumerate()
                        .map(|(index, item)| (item.id, SegmentId(index as u32)))
                        .collect(),
                    morphemes: form
                        .morphemes
                        .iter()
                        .enumerate()
                        .map(|(index, item)| (item.id, MorphemeId(index as u32)))
                        .collect(),
                    syllables: form
                        .prosody
                        .syllables
                        .iter()
                        .enumerate()
                        .map(|(index, item)| (item.id, SyllableId(index as u32)))
                        .collect(),
                    moras: form
                        .prosody
                        .moras
                        .iter()
                        .enumerate()
                        .map(|(index, item)| (item.id, MoraId(index as u32)))
                        .collect(),
                    feet: form
                        .prosody
                        .feet
                        .iter()
                        .enumerate()
                        .map(|(index, item)| (item.id, FootId(index as u32)))
                        .collect(),
                    words: form
                        .prosody
                        .words
                        .iter()
                        .enumerate()
                        .map(|(index, item)| (item.id, ProsodicWordId(index as u32)))
                        .collect(),
                };
                (form.id, maps)
            })
            .collect();

        let old_surface = self.surface_form;
        let mut normalized_forms = BTreeMap::new();
        for mut form in ordered_forms {
            let old_form = form.id;
            let maps = &local_maps[&old_form];
            form.id = form_map[&old_form];
            form.label.clear();
            for segment in &mut form.segments {
                segment.id = maps.segments[&segment.id];
                if let SegmentOrigin::Reduplicated { source } = &mut segment.origin
                    && let (Some(mapped_form), Some(source_maps)) =
                        (form_map.get(&source.form), local_maps.get(&source.form))
                    && let Some(mapped_segment) = source_maps.segments.get(&source.segment)
                {
                    source.form = *mapped_form;
                    source.segment = *mapped_segment;
                }
            }
            for morpheme in &mut form.morphemes {
                morpheme.id = maps.morphemes[&morpheme.id];
                for segment in &mut morpheme.segments {
                    if let Some(mapped) = maps.segments.get(segment) {
                        *segment = *mapped;
                    }
                }
            }
            for syllable in &mut form.prosody.syllables {
                syllable.id = maps.syllables[&syllable.id];
                for segment in syllable
                    .onset
                    .iter_mut()
                    .chain(&mut syllable.nucleus)
                    .chain(&mut syllable.coda)
                {
                    if let Some(mapped) = maps.segments.get(segment) {
                        *segment = *mapped;
                    }
                }
            }
            for mora in &mut form.prosody.moras {
                mora.id = maps.moras[&mora.id];
                if let Some(mapped) = maps.syllables.get(&mora.syllable) {
                    mora.syllable = *mapped;
                }
                for segment in &mut mora.bearers {
                    if let Some(mapped) = maps.segments.get(segment) {
                        *segment = *mapped;
                    }
                }
            }
            for foot in &mut form.prosody.feet {
                foot.id = maps.feet[&foot.id];
                for syllable in &mut foot.syllables {
                    if let Some(mapped) = maps.syllables.get(syllable) {
                        *syllable = *mapped;
                    }
                }
                if let Some(head) = &mut foot.head
                    && let Some(mapped) = maps.syllables.get(head)
                {
                    *head = *mapped;
                }
            }
            for word in &mut form.prosody.words {
                word.id = maps.words[&word.id];
                for syllable in &mut word.syllables {
                    if let Some(mapped) = maps.syllables.get(syllable) {
                        *syllable = *mapped;
                    }
                }
                for morpheme in &mut word.morphemes {
                    if let Some(mapped) = maps.morphemes.get(morpheme) {
                        *morpheme = *mapped;
                    }
                }
            }
            for tier in &mut form.tiers {
                let node_map: BTreeMap<_, _> = tier
                    .nodes
                    .iter()
                    .enumerate()
                    .map(|(index, node)| (node.id, TierNodeId(index as u32)))
                    .collect();
                for node in &mut tier.nodes {
                    node.id = node_map[&node.id];
                }
                for association in &mut tier.associations {
                    association.node = node_map[&association.node];
                    association.target = match association.target {
                        AssociationTarget::Segment(id) => AssociationTarget::Segment(
                            maps.segments.get(&id).copied().unwrap_or(id),
                        ),
                        AssociationTarget::Syllable(id) => AssociationTarget::Syllable(
                            maps.syllables.get(&id).copied().unwrap_or(id),
                        ),
                        AssociationTarget::Mora(id) => {
                            AssociationTarget::Mora(maps.moras.get(&id).copied().unwrap_or(id))
                        }
                    };
                }
                tier.associations.sort();
            }
            form.tiers.sort_by(|left, right| left.name.cmp(&right.name));
            normalized_forms.insert(form.id, form);
        }

        let mut correspondences = self.correspondences.clone();
        for graph in &mut correspondences {
            let old_source = graph.source_form;
            let old_target = graph.target_form;
            graph.source_form = form_map[&old_source];
            graph.target_form = form_map[&old_target];
            graph.label.clear();
            for link in &mut graph.links {
                for node in &mut link.source {
                    remap_correspondence_node(node, &local_maps[&old_source]);
                }
                for node in &mut link.target {
                    remap_correspondence_node(node, &local_maps[&old_target]);
                }
                link.source.sort();
                link.target.sort();
            }
            graph.links.sort_by(|left, right| {
                left.source
                    .cmp(&right.source)
                    .then_with(|| left.target.cmp(&right.target))
            });
            for (index, link) in graph.links.iter_mut().enumerate() {
                link.id = CorrespondenceLinkId(index as u32);
            }
        }
        correspondences.sort_by(|left, right| {
            left.kind
                .cmp(&right.kind)
                .then_with(|| left.source_form.cmp(&right.source_form))
                .then_with(|| left.target_form.cmp(&right.target_form))
                .then_with(|| {
                    serde_json::to_vec(&left.links)
                        .unwrap_or_default()
                        .cmp(&serde_json::to_vec(&right.links).unwrap_or_default())
                })
        });
        for (index, graph) in correspondences.iter_mut().enumerate() {
            graph.id = CorrespondenceGraphId(index as u32);
        }

        let mut derivation = self.derivation.clone();
        if !preserve_derivation {
            derivation.clear();
        } else if let Some(surface_maps) = local_maps.get(&old_surface) {
            for step in &mut derivation {
                remap_structural_edits(&mut step.edits, surface_maps);
            }
        }
        Self {
            id: CandidateId(0),
            label: String::new(),
            underlying_form: form_map[&self.underlying_form],
            surface_form: form_map[&self.surface_form],
            forms: normalized_forms,
            correspondences,
            derivation,
            metadata: self.metadata.clone(),
        }
    }
}

fn canonical_form_rank(role: &FormRole) -> u8 {
    match role {
        FormRole::Underlying => 0,
        FormRole::Surface => 1,
        FormRole::Intermediate { .. } => 2,
        FormRole::Base => 3,
        FormRole::Reduplicant => 4,
        FormRole::RelatedSurface { .. } => 5,
        FormRole::Sympathetic => 6,
        FormRole::UserNamed { .. } => 7,
    }
}

fn remap_correspondence_node(node: &mut CorrespondenceNode, maps: &impl LocalIdMapping) {
    *node = match *node {
        CorrespondenceNode::Segment(id) => CorrespondenceNode::Segment(maps.segment(id)),
        CorrespondenceNode::Morpheme(id) => CorrespondenceNode::Morpheme(maps.morpheme(id)),
        CorrespondenceNode::Syllable(id) => CorrespondenceNode::Syllable(maps.syllable(id)),
        CorrespondenceNode::Mora(id) => CorrespondenceNode::Mora(maps.mora(id)),
    };
}

trait LocalIdMapping {
    fn segment(&self, id: SegmentId) -> SegmentId;
    fn morpheme(&self, id: MorphemeId) -> MorphemeId;
    fn syllable(&self, id: SyllableId) -> SyllableId;
    fn mora(&self, id: MoraId) -> MoraId;
}

#[derive(Default)]
struct CanonicalLocalMaps {
    segments: BTreeMap<SegmentId, SegmentId>,
    morphemes: BTreeMap<MorphemeId, MorphemeId>,
    syllables: BTreeMap<SyllableId, SyllableId>,
    moras: BTreeMap<MoraId, MoraId>,
    feet: BTreeMap<FootId, FootId>,
    words: BTreeMap<ProsodicWordId, ProsodicWordId>,
}

impl LocalIdMapping for CanonicalLocalMaps {
    fn segment(&self, id: SegmentId) -> SegmentId {
        self.segments.get(&id).copied().unwrap_or(id)
    }

    fn morpheme(&self, id: MorphemeId) -> MorphemeId {
        self.morphemes.get(&id).copied().unwrap_or(id)
    }

    fn syllable(&self, id: SyllableId) -> SyllableId {
        self.syllables.get(&id).copied().unwrap_or(id)
    }

    fn mora(&self, id: MoraId) -> MoraId {
        self.moras.get(&id).copied().unwrap_or(id)
    }
}

fn remap_structural_edits(edits: &mut [StructuralEdit], maps: &CanonicalLocalMaps) {
    for edit in edits {
        match edit {
            StructuralEdit::Identity
            | StructuralEdit::Syllabify { .. }
            | StructuralEdit::AssignTone { .. } => {}
            StructuralEdit::Delete { segment, .. }
            | StructuralEdit::Insert { segment, .. }
            | StructuralEdit::FeatureChange { segment, .. } => {
                *segment = maps.segment(*segment);
            }
            StructuralEdit::Metathesis { left, right } => {
                *left = maps.segment(*left);
                *right = maps.segment(*right);
            }
            StructuralEdit::Affix { morpheme, .. } => {
                *morpheme = maps.morpheme(*morpheme);
            }
            StructuralEdit::Reduplicate { source, copies } => {
                for segment in source.iter_mut().chain(copies) {
                    *segment = maps.segment(*segment);
                }
            }
            StructuralEdit::AssignStress { primary } => {
                if let Some(syllable) = primary {
                    *syllable = maps.syllable(*syllable);
                }
            }
        }
    }
}

fn form_contains_node(form: &PhonologicalForm, node: &CorrespondenceNode) -> bool {
    match node {
        CorrespondenceNode::Segment(id) => form.segments.iter().any(|item| item.id == *id),
        CorrespondenceNode::Morpheme(id) => form.morphemes.iter().any(|item| item.id == *id),
        CorrespondenceNode::Syllable(id) => {
            form.prosody.syllables.iter().any(|item| item.id == *id)
        }
        CorrespondenceNode::Mora(id) => form.prosody.moras.iter().any(|item| item.id == *id),
    }
}

fn is_surface_family_role(role: &FormRole) -> bool {
    matches!(
        role,
        FormRole::Surface
            | FormRole::RelatedSurface { .. }
            | FormRole::Base
            | FormRole::Reduplicant
            | FormRole::Sympathetic
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StructureIssueCode {
    DuplicateId,
    DanglingReference,
    MissingPrimaryForm,
    MissingCorrespondence,
    EmptyCorrespondenceLink,
    MismatchedId,
    WrongFormRole,
    InvalidProsody,
    InvalidNumericValue,
    IdSpaceExhausted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructureIssue {
    pub code: StructureIssueCode,
    pub path: String,
    pub message: String,
}

impl StructureIssue {
    fn new(code: StructureIssueCode, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code,
            path: path.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructureError {
    pub issues: Vec<StructureIssue>,
}

impl StructureError {
    fn single(
        code: StructureIssueCode,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            issues: vec![StructureIssue::new(code, path, message)],
        }
    }
}

impl fmt::Display for StructureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, issue) in self.issues.iter().enumerate() {
            if index > 0 {
                write!(formatter, "; ")?;
            }
            write!(formatter, "{}: {}", issue.path, issue.message)?;
        }
        Ok(())
    }
}

impl Error for StructureError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SegmentSelector {
    All,
    At {
        positions: Vec<usize>,
    },
    Symbol {
        symbol: String,
    },
    Feature {
        name: FeatureName,
        value: FeatureValue,
    },
    MorphemeKind {
        morpheme_kind: MorphemeKind,
    },
    And {
        selectors: Vec<SegmentSelector>,
    },
    Or {
        selectors: Vec<SegmentSelector>,
    },
    Not {
        selector: Box<SegmentSelector>,
    },
}

impl SegmentSelector {
    fn matches(&self, form: &PhonologicalForm, index: usize) -> bool {
        let Some(segment) = form.segments.get(index) else {
            return false;
        };
        match self {
            Self::All => true,
            Self::At { positions } => positions.contains(&index),
            Self::Symbol { symbol } => &segment.symbol == symbol,
            Self::Feature { name, value } => segment.features.get(name) == Some(value),
            Self::MorphemeKind { morpheme_kind } => form.morphemes.iter().any(|morpheme| {
                &morpheme.kind == morpheme_kind && morpheme.segments.contains(&segment.id)
            }),
            Self::And { selectors } => selectors.iter().all(|item| item.matches(form, index)),
            Self::Or { selectors } => selectors.iter().any(|item| item.matches(form, index)),
            Self::Not { selector } => !selector.matches(form, index),
        }
    }

    fn positions(&self, form: &PhonologicalForm) -> Vec<usize> {
        (0..form.segments.len())
            .filter(|index| self.matches(form, *index))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum InsertionSites {
    EveryBoundary,
    Initial,
    Final,
    At { boundaries: Vec<usize> },
    Before { selector: SegmentSelector },
    After { selector: SegmentSelector },
}

impl InsertionSites {
    fn boundaries(&self, form: &PhonologicalForm) -> Vec<usize> {
        let mut values = match self {
            Self::EveryBoundary => (0..=form.segments.len()).collect(),
            Self::Initial => vec![0],
            Self::Final => vec![form.segments.len()],
            Self::At { boundaries } => boundaries
                .iter()
                .copied()
                .filter(|index| *index <= form.segments.len())
                .collect(),
            Self::Before { selector } => selector.positions(form),
            Self::After { selector } => selector
                .positions(form)
                .into_iter()
                .map(|index| index + 1)
                .collect(),
        };
        values.sort_unstable();
        values.dedup();
        values
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AffixSite {
    Prefix,
    Suffix,
    At { boundaries: Vec<usize> },
    AfterFirst { selector: SegmentSelector },
}

impl AffixSite {
    fn boundaries(&self, form: &PhonologicalForm) -> Vec<usize> {
        match self {
            Self::Prefix => vec![0],
            Self::Suffix => vec![form.segments.len()],
            Self::At { boundaries } => {
                let mut values: Vec<_> = boundaries
                    .iter()
                    .copied()
                    .filter(|index| *index <= form.segments.len())
                    .collect();
                values.sort_unstable();
                values.dedup();
                values
            }
            Self::AfterFirst { selector } => selector
                .positions(form)
                .first()
                .map(|index| vec![index + 1])
                .unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ReduplicationDomain {
    WholeForm,
    SegmentRange { start: usize, end_exclusive: usize },
    FirstSyllable,
    Morpheme { label: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReduplicationSite {
    Prefix,
    Suffix,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyllabificationSpec {
    pub nucleus_selector: SegmentSelector,
    pub max_onset: usize,
    pub max_coda: usize,
    #[serde(default)]
    pub allow_empty_onset: bool,
    #[serde(default)]
    pub allow_empty_coda: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StressPosition {
    Any,
    Initial,
    Final,
    Penultimate,
    Index(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SecondaryStressPolicy {
    None,
    AlternatingLeftToRight,
    AlternatingRightToLeft,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StressAssignmentSpec {
    pub primary: StressPosition,
    pub secondary: SecondaryStressPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToneTarget {
    Segments,
    Syllables,
    Moras,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TonePattern {
    OnePerTarget,
    SpreadSingle,
    Floating,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToneAssignmentSpec {
    pub tier_name: String,
    pub inventory: Vec<ToneValue>,
    pub targets: ToneTarget,
    pub pattern: TonePattern,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum GenerationOperation {
    Identity,
    Delete {
        selector: SegmentSelector,
    },
    Insert {
        inventory: Vec<SegmentTemplate>,
        sites: InsertionSites,
    },
    FeatureChange {
        selector: SegmentSelector,
        feature: FeatureName,
        values: Vec<FeatureValue>,
    },
    Metathesis {
        selector: SegmentSelector,
        max_distance: usize,
    },
    Affix {
        morpheme: MorphemeTemplate,
        site: AffixSite,
    },
    Reduplicate {
        domain: ReduplicationDomain,
        site: ReduplicationSite,
    },
    Syllabify {
        specification: SyllabificationSpec,
    },
    AssignStress {
        specification: StressAssignmentSpec,
    },
    AssignTone {
        specification: ToneAssignmentSpec,
    },
}

impl GenerationOperation {
    pub const fn class(&self) -> OperationClass {
        match self {
            Self::Identity => OperationClass::Identity,
            Self::Delete { .. } => OperationClass::Deletion,
            Self::Insert { .. } => OperationClass::Insertion,
            Self::FeatureChange { .. } => OperationClass::FeatureChange,
            Self::Metathesis { .. } => OperationClass::Metathesis,
            Self::Affix { .. } => OperationClass::Affixation,
            Self::Reduplicate { .. } => OperationClass::Reduplication,
            Self::Syllabify { .. } => OperationClass::Syllabification,
            Self::AssignStress { .. } => OperationClass::Stress,
            Self::AssignTone { .. } => OperationClass::Tone,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OperationClass {
    Identity,
    Deletion,
    Insertion,
    FeatureChange,
    Metathesis,
    Affixation,
    Reduplication,
    Syllabification,
    Stress,
    Tone,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationDeclaration {
    pub id: String,
    pub operation: GenerationOperation,
    /// A semantic bound in the declared finite `GEN`, not a timeout.
    pub max_applications_per_candidate: u16,
}

impl OperationDeclaration {
    pub fn once(id: impl Into<String>, operation: GenerationOperation) -> Self {
        Self {
            id: id.into(),
            operation,
            max_applications_per_candidate: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SupportClaim {
    /// Exhaustive enumeration is claimed only for `GeneratorSpec::domain`.
    CompleteForDeclaredDomain { statement: String },
    /// An external verifier owns this proof obligation. `FiniteGenerator`
    /// checks that the identifier and statement are present; integration must
    /// verify the referenced certificate before advertising certified support.
    CompleteByCertificate {
        certificate_id: String,
        statement: String,
    },
    /// A deliberately partial support for exploration. Exhausting the local
    /// operation list still returns `CompletenessStatus::Truncated`.
    Exploratory,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeduplicationPolicy {
    /// Suitable only when no constraint or query consumes correspondence,
    /// prosody, morphology, or derivational structure.
    SurfaceForm,
    /// Default for parallel analyses: preserve every declared structure and
    /// correspondence, but collapse duplicate construction histories.
    #[default]
    StructuredRepresentation,
    /// Keep structurally identical candidates distinct when their declared
    /// derivations are themselves part of the answer type.
    PreserveDerivations,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclaredGenerationDomain {
    pub max_derivation_steps: usize,
    pub max_segments_per_form: usize,
}

impl Default for DeclaredGenerationDomain {
    fn default() -> Self {
        Self {
            max_derivation_steps: 4,
            max_segments_per_form: 64,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationResources {
    pub max_candidates: usize,
    pub max_operation_expansions: usize,
    pub max_variants_per_application: usize,
}

impl Default for GenerationResources {
    fn default() -> Self {
        Self {
            max_candidates: 100_000,
            max_operation_expansions: 1_000_000,
            max_variants_per_application: 100_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratorSpec {
    pub name: String,
    pub operations: Vec<OperationDeclaration>,
    pub domain: DeclaredGenerationDomain,
    pub resources: GenerationResources,
    pub support_claim: SupportClaim,
    #[serde(default)]
    pub deduplication: DeduplicationPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SupportDependentClaim {
    CompleteCandidateSupport,
    CompleteCandidateOrder,
    WinnerUnderUnrestrictedGen,
    FactorialTypology,
    MaximumEntropyLaw,
    HarmonicBounding,
    SecondOrderSupportComparison,
}

fn all_support_claims() -> Vec<SupportDependentClaim> {
    vec![
        SupportDependentClaim::CompleteCandidateSupport,
        SupportDependentClaim::CompleteCandidateOrder,
        SupportDependentClaim::WinnerUnderUnrestrictedGen,
        SupportDependentClaim::FactorialTypology,
        SupportDependentClaim::MaximumEntropyLaw,
        SupportDependentClaim::HarmonicBounding,
        SupportDependentClaim::SecondOrderSupportComparison,
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GenerationReasonCode {
    InvalidInput,
    InvalidGenerator,
    DuplicateOperationId,
    EmptyInventory,
    EmptyCertificate,
    CandidateLimit,
    ExpansionLimit,
    VariantLimit,
    ExploratorySupport,
    IdSpaceExhausted,
    InvalidGeneratedCandidate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationReason {
    pub code: GenerationReasonCode,
    pub operation_id: Option<String>,
    pub coordinate: String,
    pub message: String,
    pub unfinished_domain: Option<String>,
    pub blocked_claims: Vec<SupportDependentClaim>,
}

impl GenerationReason {
    fn new(
        code: GenerationReasonCode,
        operation_id: Option<String>,
        coordinate: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            operation_id,
            coordinate: coordinate.into(),
            message: message.into(),
            unfinished_domain: None,
            blocked_claims: all_support_claims(),
        }
    }

    fn unfinished(mut self, domain: impl Into<String>) -> Self {
        self.unfinished_domain = Some(domain.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum CompletenessStatus {
    Complete { claim: SupportClaim },
    Truncated { reasons: Vec<GenerationReason> },
    Refused { reasons: Vec<GenerationReason> },
}

impl CompletenessStatus {
    pub const fn is_complete(&self) -> bool {
        matches!(self, Self::Complete { .. })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationStatistics {
    pub retained_candidates: usize,
    pub operation_expansions: usize,
    pub variants_considered: usize,
    pub duplicates_removed: usize,
    pub peak_queue: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationResult {
    pub candidates: Vec<StructuredCandidate>,
    pub status: CompletenessStatus,
    pub statistics: GenerationStatistics,
}

impl GenerationResult {
    pub fn require_complete(&self) -> Result<&[StructuredCandidate], &[GenerationReason]> {
        match &self.status {
            CompletenessStatus::Complete { .. } => Ok(&self.candidates),
            CompletenessStatus::Truncated { reasons } | CompletenessStatus::Refused { reasons } => {
                Err(reasons)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FiniteGenerator;

impl FiniteGenerator {
    pub fn generate(input: &UnderlyingForm, spec: &GeneratorSpec) -> GenerationResult {
        let validation = validate_generator(input, spec);
        if !validation.is_empty() {
            return GenerationResult {
                candidates: Vec::new(),
                status: CompletenessStatus::Refused {
                    reasons: validation,
                },
                statistics: GenerationStatistics::default(),
            };
        }

        let initial = StructuredCandidate::identity(input);
        let initial_key = search_state_key(&initial, spec.deduplication);
        let mut retained = BTreeMap::from([(initial_key.clone(), initial)]);
        let mut queue = VecDeque::from([initial_key]);
        let mut statistics = GenerationStatistics {
            retained_candidates: 1,
            peak_queue: 1,
            ..GenerationStatistics::default()
        };
        let mut truncation = Vec::new();

        'search: while let Some(key) = queue.pop_front() {
            let candidate = retained
                .get(&key)
                .expect("generation queue keys are retained")
                .clone();
            if candidate.derivation.len() >= spec.domain.max_derivation_steps {
                continue;
            }
            for declaration in &spec.operations {
                if declaration.operation == GenerationOperation::Identity {
                    continue;
                }
                let used = candidate
                    .derivation
                    .iter()
                    .filter(|step| step.operation_id == declaration.id)
                    .count();
                if used >= usize::from(declaration.max_applications_per_candidate) {
                    continue;
                }
                if statistics.operation_expansions >= spec.resources.max_operation_expansions {
                    truncation.push(
                        GenerationReason::new(
                            GenerationReasonCode::ExpansionLimit,
                            Some(declaration.id.clone()),
                            "generator.resources.max-operation-expansions",
                            "the deterministic operation-expansion budget was exhausted",
                        )
                        .unfinished(format!(
                            "candidate {} and later queued candidates",
                            candidate.surface_string()
                        )),
                    );
                    break 'search;
                }
                statistics.operation_expansions += 1;
                let application = match apply_operation(
                    &candidate,
                    declaration,
                    &spec.domain,
                    spec.resources.max_variants_per_application,
                ) {
                    Ok(value) => value,
                    Err(reason) => {
                        return GenerationResult {
                            candidates: assign_candidate_ids(retained, spec.deduplication),
                            status: CompletenessStatus::Refused {
                                reasons: vec![reason],
                            },
                            statistics,
                        };
                    }
                };
                if let Some(reason) = application.truncated {
                    truncation.push(reason);
                    break 'search;
                }
                for variant in application.candidates {
                    statistics.variants_considered += 1;
                    let structural_issues = variant.validate();
                    if !structural_issues.is_empty() {
                        let reasons = structural_issues
                            .into_iter()
                            .map(|issue| {
                                GenerationReason::new(
                                    GenerationReasonCode::InvalidGeneratedCandidate,
                                    Some(declaration.id.clone()),
                                    issue.path,
                                    issue.message,
                                )
                            })
                            .collect();
                        return GenerationResult {
                            candidates: assign_candidate_ids(retained, spec.deduplication),
                            status: CompletenessStatus::Refused { reasons },
                            statistics,
                        };
                    }
                    let variant_key = search_state_key(&variant, spec.deduplication);
                    if retained.contains_key(&variant_key) {
                        statistics.duplicates_removed += 1;
                        continue;
                    }
                    if retained.len() >= spec.resources.max_candidates {
                        truncation.push(
                            GenerationReason::new(
                                GenerationReasonCode::CandidateLimit,
                                Some(declaration.id.clone()),
                                "generator.resources.max-candidates",
                                "the candidate budget was exhausted before the finite domain was enumerated",
                            )
                            .unfinished(format!(
                                "variants reachable after {} on {}",
                                declaration.id,
                                candidate.surface_string()
                            )),
                        );
                        break 'search;
                    }
                    queue.push_back(variant_key.clone());
                    retained.insert(variant_key, variant);
                    statistics.peak_queue = statistics.peak_queue.max(queue.len());
                }
            }
        }

        let candidates = assign_candidate_ids(retained, spec.deduplication);
        statistics.retained_candidates = candidates.len();
        let status = if !truncation.is_empty() {
            CompletenessStatus::Truncated {
                reasons: truncation,
            }
        } else {
            match &spec.support_claim {
                SupportClaim::Exploratory => CompletenessStatus::Truncated {
                    reasons: vec![GenerationReason::new(
                        GenerationReasonCode::ExploratorySupport,
                        None,
                        "generator.support-claim",
                        "the declared support is exploratory and therefore cannot certify completeness",
                    )],
                },
                claim => CompletenessStatus::Complete {
                    claim: claim.clone(),
                },
            }
        };
        GenerationResult {
            candidates,
            status,
            statistics,
        }
    }
}

fn validate_generator(input: &UnderlyingForm, spec: &GeneratorSpec) -> Vec<GenerationReason> {
    let mut reasons = input
        .0
        .validate()
        .into_iter()
        .map(|issue| {
            GenerationReason::new(
                GenerationReasonCode::InvalidInput,
                None,
                issue.path,
                issue.message,
            )
        })
        .collect::<Vec<_>>();
    if input.0.role != FormRole::Underlying {
        reasons.push(GenerationReason::new(
            GenerationReasonCode::InvalidInput,
            None,
            "generator.input.role",
            "the generator input must be an underlying form",
        ));
    }
    if spec.domain.max_segments_per_form == 0
        || spec.resources.max_candidates == 0
        || spec.resources.max_operation_expansions == 0
        || spec.resources.max_variants_per_application == 0
    {
        reasons.push(GenerationReason::new(
            GenerationReasonCode::InvalidGenerator,
            None,
            "generator.bounds",
            "segment, candidate, expansion, and per-application bounds must be positive",
        ));
    }
    if input.0.segments.len() > spec.domain.max_segments_per_form {
        reasons.push(GenerationReason::new(
            GenerationReasonCode::InvalidInput,
            None,
            "generator.input.segments",
            "the input already exceeds the declared segment bound",
        ));
    }
    let mut operation_ids = BTreeSet::new();
    for declaration in &spec.operations {
        if declaration.id.trim().is_empty() {
            reasons.push(GenerationReason::new(
                GenerationReasonCode::InvalidGenerator,
                None,
                "generator.operations.id",
                "every operation declaration requires a nonempty stable identifier",
            ));
        } else if !operation_ids.insert(declaration.id.clone()) {
            reasons.push(GenerationReason::new(
                GenerationReasonCode::DuplicateOperationId,
                Some(declaration.id.clone()),
                "generator.operations.id",
                "operation identifiers must be unique",
            ));
        }
        match &declaration.operation {
            GenerationOperation::Insert { inventory, .. } if inventory.is_empty() => {
                reasons.push(GenerationReason::new(
                    GenerationReasonCode::EmptyInventory,
                    Some(declaration.id.clone()),
                    "generator.operations.insert.inventory",
                    "insertion requires a declared nonempty segment inventory",
                ));
            }
            GenerationOperation::FeatureChange { values, .. } if values.is_empty() => {
                reasons.push(GenerationReason::new(
                    GenerationReasonCode::EmptyInventory,
                    Some(declaration.id.clone()),
                    "generator.operations.feature-change.values",
                    "feature change requires a declared nonempty value inventory",
                ));
            }
            GenerationOperation::Affix { morpheme, .. } if morpheme.segments.is_empty() => {
                reasons.push(GenerationReason::new(
                    GenerationReasonCode::EmptyInventory,
                    Some(declaration.id.clone()),
                    "generator.operations.affix.morpheme.segments",
                    "affixation requires at least one declared segment",
                ));
            }
            GenerationOperation::AssignTone { specification }
                if specification.inventory.is_empty() =>
            {
                reasons.push(GenerationReason::new(
                    GenerationReasonCode::EmptyInventory,
                    Some(declaration.id.clone()),
                    "generator.operations.assign-tone.inventory",
                    "tone assignment requires a declared nonempty tone inventory",
                ));
            }
            GenerationOperation::Metathesis {
                max_distance: 0, ..
            } => {
                reasons.push(GenerationReason::new(
                    GenerationReasonCode::InvalidGenerator,
                    Some(declaration.id.clone()),
                    "generator.operations.metathesis.max-distance",
                    "metathesis distance must be positive",
                ));
            }
            GenerationOperation::Syllabify { specification }
                if specification.max_onset == 0 && !specification.allow_empty_onset =>
            {
                reasons.push(GenerationReason::new(
                    GenerationReasonCode::InvalidGenerator,
                    Some(declaration.id.clone()),
                    "generator.operations.syllabify.max-onset",
                    "a zero onset bound requires empty onsets to be licensed",
                ));
            }
            _ => {}
        }
    }
    if let SupportClaim::CompleteByCertificate {
        certificate_id,
        statement,
    } = &spec.support_claim
        && (certificate_id.trim().is_empty() || statement.trim().is_empty())
    {
        reasons.push(GenerationReason::new(
            GenerationReasonCode::EmptyCertificate,
            None,
            "generator.support-claim.certificate",
            "a completeness certificate requires a nonempty identifier and statement",
        ));
    }
    reasons
}

#[derive(Debug)]
struct OperationApplication {
    candidates: Vec<StructuredCandidate>,
    truncated: Option<GenerationReason>,
}

impl OperationApplication {
    fn empty() -> Self {
        Self {
            candidates: Vec::new(),
            truncated: None,
        }
    }
}

fn apply_operation(
    candidate: &StructuredCandidate,
    declaration: &OperationDeclaration,
    domain: &DeclaredGenerationDomain,
    variant_limit: usize,
) -> Result<OperationApplication, GenerationReason> {
    match &declaration.operation {
        GenerationOperation::Identity => Ok(OperationApplication::empty()),
        GenerationOperation::Delete { selector } => {
            apply_deletion(candidate, declaration, selector, variant_limit)
        }
        GenerationOperation::Insert { inventory, sites } => apply_insertion(
            candidate,
            declaration,
            inventory,
            sites,
            domain,
            variant_limit,
        ),
        GenerationOperation::FeatureChange {
            selector,
            feature,
            values,
        } => apply_feature_change(
            candidate,
            declaration,
            selector,
            feature,
            values,
            variant_limit,
        ),
        GenerationOperation::Metathesis {
            selector,
            max_distance,
        } => apply_metathesis(
            candidate,
            declaration,
            selector,
            *max_distance,
            variant_limit,
        ),
        GenerationOperation::Affix { morpheme, site } => apply_affixation(
            candidate,
            declaration,
            morpheme,
            site,
            domain,
            variant_limit,
        ),
        GenerationOperation::Reduplicate { domain: copy, site } => {
            apply_reduplication(candidate, declaration, copy, *site, domain, variant_limit)
        }
        GenerationOperation::Syllabify { specification } => {
            apply_syllabification(candidate, declaration, specification, variant_limit)
        }
        GenerationOperation::AssignStress { specification } => {
            apply_stress(candidate, declaration, specification, variant_limit)
        }
        GenerationOperation::AssignTone { specification } => {
            apply_tone(candidate, declaration, specification, variant_limit)
        }
    }
}

fn push_variant(
    application: &mut OperationApplication,
    mut candidate: StructuredCandidate,
    declaration: &OperationDeclaration,
    edits: Vec<StructuralEdit>,
    limit: usize,
) -> bool {
    if application.candidates.len() >= limit {
        application.truncated = Some(
            GenerationReason::new(
                GenerationReasonCode::VariantLimit,
                Some(declaration.id.clone()),
                "generator.resources.max-variants-per-application",
                "one operation application exceeded its deterministic variant budget",
            )
            .unfinished("remaining variants of this operation application"),
        );
        return false;
    }
    candidate.id = CandidateId(0);
    candidate.label = candidate.surface_string();
    candidate.derivation.push(DerivationStep {
        operation_id: declaration.id.clone(),
        operation_class: declaration.operation.class(),
        edits,
    });
    application.candidates.push(candidate);
    true
}

fn apply_deletion(
    candidate: &StructuredCandidate,
    declaration: &OperationDeclaration,
    selector: &SegmentSelector,
    variant_limit: usize,
) -> Result<OperationApplication, GenerationReason> {
    let positions = selector.positions(candidate.surface());
    let mut application = OperationApplication::empty();
    for position in positions {
        let mut variant = candidate.clone();
        let segment = variant.surface().segments[position].id;
        {
            let surface = variant.surface_mut();
            surface.segments.remove(position);
            for morpheme in &mut surface.morphemes {
                morpheme.segments.retain(|item| *item != segment);
            }
            surface.prosody.remove_segment(segment);
            let remaining_syllables: BTreeSet<_> = surface
                .prosody
                .syllables
                .iter()
                .map(|item| item.id)
                .collect();
            let remaining_moras: BTreeSet<_> =
                surface.prosody.moras.iter().map(|item| item.id).collect();
            for tier in &mut surface.tiers {
                tier.associations
                    .retain(|association| match association.target {
                        AssociationTarget::Segment(id) => id != segment,
                        AssociationTarget::Syllable(id) => remaining_syllables.contains(&id),
                        AssociationTarget::Mora(id) => remaining_moras.contains(&id),
                    });
            }
        }
        variant.remove_surface_segment_from_correspondence(segment);
        if !push_variant(
            &mut application,
            variant,
            declaration,
            vec![StructuralEdit::Delete { segment, position }],
            variant_limit,
        ) {
            break;
        }
    }
    Ok(application)
}

fn apply_insertion(
    candidate: &StructuredCandidate,
    declaration: &OperationDeclaration,
    inventory: &[SegmentTemplate],
    sites: &InsertionSites,
    domain: &DeclaredGenerationDomain,
    variant_limit: usize,
) -> Result<OperationApplication, GenerationReason> {
    if candidate.surface().segments.len() >= domain.max_segments_per_form {
        return Ok(OperationApplication::empty());
    }
    let boundaries = sites.boundaries(candidate.surface());
    let mut application = OperationApplication::empty();
    for position in boundaries {
        for template in inventory {
            let mut variant = candidate.clone();
            let segment = variant.surface().next_segment_id().ok_or_else(|| {
                generation_structure_reason(
                    declaration,
                    "surface.segments",
                    "segment identifier space exhausted",
                )
            })?;
            variant.surface_mut().segments.insert(
                position,
                Segment::from_template(segment, template, SegmentOrigin::Inserted),
            );
            variant
                .register_surface_insertion(segment)
                .map_err(|error| {
                    generation_structure_reason(
                        declaration,
                        "candidate.correspondences.io",
                        error.to_string(),
                    )
                })?;
            if !push_variant(
                &mut application,
                variant,
                declaration,
                vec![StructuralEdit::Insert { segment, position }],
                variant_limit,
            ) {
                return Ok(application);
            }
        }
    }
    Ok(application)
}

fn apply_feature_change(
    candidate: &StructuredCandidate,
    declaration: &OperationDeclaration,
    selector: &SegmentSelector,
    feature: &FeatureName,
    values: &[FeatureValue],
    variant_limit: usize,
) -> Result<OperationApplication, GenerationReason> {
    let positions = selector.positions(candidate.surface());
    let mut application = OperationApplication::empty();
    for position in positions {
        for value in values {
            let current = candidate.surface().segments[position]
                .features
                .get(feature)
                .cloned();
            if current.as_ref() == Some(value) {
                continue;
            }
            let mut variant = candidate.clone();
            let segment = variant.surface().segments[position].id;
            variant.surface_mut().segments[position]
                .features
                .insert(feature.clone(), value.clone());
            if !push_variant(
                &mut application,
                variant,
                declaration,
                vec![StructuralEdit::FeatureChange {
                    segment,
                    feature: feature.clone(),
                    from: current,
                    to: value.clone(),
                }],
                variant_limit,
            ) {
                return Ok(application);
            }
        }
    }
    Ok(application)
}

fn apply_metathesis(
    candidate: &StructuredCandidate,
    declaration: &OperationDeclaration,
    selector: &SegmentSelector,
    max_distance: usize,
    variant_limit: usize,
) -> Result<OperationApplication, GenerationReason> {
    let positions = selector.positions(candidate.surface());
    let mut application = OperationApplication::empty();
    for (left_index, left) in positions.iter().enumerate() {
        for right in positions.iter().skip(left_index + 1) {
            if right - left > max_distance {
                continue;
            }
            let mut variant = candidate.clone();
            let left_id = variant.surface().segments[*left].id;
            let right_id = variant.surface().segments[*right].id;
            variant.surface_mut().segments.swap(*left, *right);
            if !push_variant(
                &mut application,
                variant,
                declaration,
                vec![StructuralEdit::Metathesis {
                    left: left_id,
                    right: right_id,
                }],
                variant_limit,
            ) {
                return Ok(application);
            }
        }
    }
    Ok(application)
}

fn apply_affixation(
    candidate: &StructuredCandidate,
    declaration: &OperationDeclaration,
    template: &MorphemeTemplate,
    site: &AffixSite,
    domain: &DeclaredGenerationDomain,
    variant_limit: usize,
) -> Result<OperationApplication, GenerationReason> {
    if candidate.surface().segments.len() + template.segments.len() > domain.max_segments_per_form {
        return Ok(OperationApplication::empty());
    }
    let boundaries = site.boundaries(candidate.surface());
    let mut application = OperationApplication::empty();
    for position in boundaries {
        let mut variant = candidate.clone();
        let morpheme = variant.surface().next_morpheme_id().ok_or_else(|| {
            generation_structure_reason(
                declaration,
                "surface.morphemes",
                "morpheme identifier space exhausted",
            )
        })?;
        let mut segment_ids = Vec::with_capacity(template.segments.len());
        for (offset, segment_template) in template.segments.iter().enumerate() {
            let segment = variant.surface().next_segment_id().ok_or_else(|| {
                generation_structure_reason(
                    declaration,
                    "surface.segments",
                    "segment identifier space exhausted",
                )
            })?;
            variant.surface_mut().segments.insert(
                position + offset,
                Segment::from_template(
                    segment,
                    segment_template,
                    SegmentOrigin::Affix {
                        morpheme: template.label.clone(),
                    },
                ),
            );
            variant
                .register_surface_insertion(segment)
                .map_err(|error| {
                    generation_structure_reason(
                        declaration,
                        "candidate.correspondences.io",
                        error.to_string(),
                    )
                })?;
            segment_ids.push(segment);
        }
        variant.surface_mut().morphemes.push(Morpheme {
            id: morpheme,
            label: template.label.clone(),
            kind: template.kind.clone(),
            segments: segment_ids,
        });
        if !push_variant(
            &mut application,
            variant,
            declaration,
            vec![StructuralEdit::Affix { morpheme, position }],
            variant_limit,
        ) {
            return Ok(application);
        }
    }
    Ok(application)
}

fn reduplication_source_positions(
    form: &PhonologicalForm,
    domain: &ReduplicationDomain,
) -> Vec<usize> {
    match domain {
        ReduplicationDomain::WholeForm => (0..form.segments.len()).collect(),
        ReduplicationDomain::SegmentRange {
            start,
            end_exclusive,
        } => (*start..(*end_exclusive).min(form.segments.len())).collect(),
        ReduplicationDomain::FirstSyllable => form
            .prosody
            .syllables
            .first()
            .map(|syllable| {
                let ids: BTreeSet<_> = syllable.segment_ids().collect();
                form.segments
                    .iter()
                    .enumerate()
                    .filter_map(|(index, segment)| ids.contains(&segment.id).then_some(index))
                    .collect()
            })
            .unwrap_or_default(),
        ReduplicationDomain::Morpheme { label } => form
            .morphemes
            .iter()
            .find(|morpheme| &morpheme.label == label)
            .map(|morpheme| {
                let ids: BTreeSet<_> = morpheme.segments.iter().copied().collect();
                form.segments
                    .iter()
                    .enumerate()
                    .filter_map(|(index, segment)| ids.contains(&segment.id).then_some(index))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn apply_reduplication(
    candidate: &StructuredCandidate,
    declaration: &OperationDeclaration,
    copy_domain: &ReduplicationDomain,
    site: ReduplicationSite,
    domain: &DeclaredGenerationDomain,
    variant_limit: usize,
) -> Result<OperationApplication, GenerationReason> {
    let positions = reduplication_source_positions(candidate.surface(), copy_domain);
    if positions.is_empty()
        || candidate.surface().segments.len() + positions.len() > domain.max_segments_per_form
    {
        return Ok(OperationApplication::empty());
    }
    let source_segments: Vec<_> = positions
        .iter()
        .map(|position| candidate.surface().segments[*position].clone())
        .collect();
    let mut variant = candidate.clone();
    let insertion = match site {
        ReduplicationSite::Prefix => 0,
        ReduplicationSite::Suffix => variant.surface().segments.len(),
    };
    let surface_id = variant.surface_form;
    let mut copy_ids = Vec::with_capacity(source_segments.len());
    for (offset, source) in source_segments.iter().enumerate() {
        let copy = variant.surface().next_segment_id().ok_or_else(|| {
            generation_structure_reason(
                declaration,
                "surface.segments",
                "segment identifier space exhausted",
            )
        })?;
        let mut copied = source.clone();
        copied.id = copy;
        copied.origin = SegmentOrigin::Reduplicated {
            source: SegmentReference {
                form: surface_id,
                segment: source.id,
            },
        };
        variant
            .surface_mut()
            .segments
            .insert(insertion + offset, copied);
        variant.register_surface_insertion(copy).map_err(|error| {
            generation_structure_reason(
                declaration,
                "candidate.correspondences.io",
                error.to_string(),
            )
        })?;
        copy_ids.push(copy);
    }

    let base_form_id = variant
        .forms
        .keys()
        .map(|id| id.0)
        .max()
        .unwrap_or(1)
        .checked_add(1)
        .map(FormId)
        .ok_or_else(|| {
            generation_structure_reason(
                declaration,
                "candidate.forms",
                "form identifier space exhausted",
            )
        })?;
    let reduplicant_form_id = base_form_id.0.checked_add(1).map(FormId).ok_or_else(|| {
        generation_structure_reason(
            declaration,
            "candidate.forms",
            "form identifier space exhausted",
        )
    })?;
    let mut base_form = PhonologicalForm {
        id: base_form_id,
        label: "base".into(),
        role: FormRole::Base,
        segments: source_segments.clone(),
        morphemes: Vec::new(),
        prosody: ProsodicStructure::default(),
        tiers: Vec::new(),
    };
    for (index, segment) in base_form.segments.iter_mut().enumerate() {
        segment.id = SegmentId(index as u32);
    }
    let mut reduplicant_form = base_form.clone();
    reduplicant_form.id = reduplicant_form_id;
    reduplicant_form.label = "reduplicant".into();
    reduplicant_form.role = FormRole::Reduplicant;
    let br = CorrespondenceGraph::identity_segments(
        variant.next_graph_id().ok_or_else(|| {
            generation_structure_reason(
                declaration,
                "candidate.correspondences",
                "correspondence graph identifier space exhausted",
            )
        })?,
        "BR",
        CorrespondenceKind::BaseReduplicant,
        &base_form,
        &reduplicant_form,
    );
    variant.forms.insert(base_form_id, base_form);
    variant.forms.insert(reduplicant_form_id, reduplicant_form);
    variant.correspondences.push(br);

    let mut application = OperationApplication::empty();
    push_variant(
        &mut application,
        variant,
        declaration,
        vec![StructuralEdit::Reduplicate {
            source: source_segments.iter().map(|segment| segment.id).collect(),
            copies: copy_ids,
        }],
        variant_limit,
    );
    Ok(application)
}

fn apply_syllabification(
    candidate: &StructuredCandidate,
    declaration: &OperationDeclaration,
    specification: &SyllabificationSpec,
    variant_limit: usize,
) -> Result<OperationApplication, GenerationReason> {
    let form = candidate.surface();
    let nuclei = specification.nucleus_selector.positions(form);
    if nuclei.is_empty() {
        return Ok(OperationApplication::empty());
    }
    let initial_count = nuclei[0];
    let final_count = form.segments.len() - nuclei[nuclei.len() - 1] - 1;
    if initial_count > specification.max_onset || final_count > specification.max_coda {
        return Ok(OperationApplication::empty());
    }
    if (!specification.allow_empty_onset && initial_count == 0)
        || (!specification.allow_empty_coda && final_count == 0)
    {
        return Ok(OperationApplication::empty());
    }

    let mut split_options = Vec::new();
    for pair in nuclei.windows(2) {
        let between = pair[1] - pair[0] - 1;
        let options: Vec<_> = (0..=between)
            .filter(|coda| {
                *coda <= specification.max_coda
                    && between - *coda <= specification.max_onset
                    && (specification.allow_empty_coda || *coda > 0)
                    && (specification.allow_empty_onset || between - *coda > 0)
            })
            .collect();
        if options.is_empty() {
            return Ok(OperationApplication::empty());
        }
        split_options.push(options);
    }
    let mut combinations = Vec::new();
    enumerate_usize_product(
        &split_options,
        0,
        &mut Vec::new(),
        &mut combinations,
        variant_limit.saturating_add(1),
    );
    let mut application = OperationApplication::empty();
    if combinations.len() > variant_limit {
        application.truncated = Some(
            GenerationReason::new(
                GenerationReasonCode::VariantLimit,
                Some(declaration.id.clone()),
                "generator.operations.syllabify",
                "syllabification alternatives exceeded the per-application budget",
            )
            .unfinished("remaining inter-nuclear onset/coda splits"),
        );
        return Ok(application);
    }
    for combination in combinations {
        let mut syllables = Vec::with_capacity(nuclei.len());
        for (syllable_index, nucleus_position) in nuclei.iter().copied().enumerate() {
            let onset_start = if syllable_index == 0 {
                0
            } else {
                let previous_nucleus = nuclei[syllable_index - 1];
                previous_nucleus + 1 + combination[syllable_index - 1]
            };
            let coda_end = if syllable_index + 1 == nuclei.len() {
                form.segments.len()
            } else {
                nucleus_position + 1 + combination[syllable_index]
            };
            syllables.push(Syllable {
                id: SyllableId(syllable_index as u32),
                onset: form.segments[onset_start..nucleus_position]
                    .iter()
                    .map(|segment| segment.id)
                    .collect(),
                nucleus: vec![form.segments[nucleus_position].id],
                coda: form.segments[nucleus_position + 1..coda_end]
                    .iter()
                    .map(|segment| segment.id)
                    .collect(),
                stress: StressLevel::Unstressed,
            });
        }
        let mut variant = candidate.clone();
        let syllable_ids = syllables.iter().map(|item| item.id).collect();
        let morphemes = variant
            .surface()
            .morphemes
            .iter()
            .map(|item| item.id)
            .collect();
        variant.surface_mut().prosody = ProsodicStructure {
            syllables,
            moras: Vec::new(),
            feet: Vec::new(),
            words: vec![ProsodicWord {
                id: ProsodicWordId(0),
                syllables: syllable_ids,
                morphemes,
            }],
        };
        let count = variant.surface().prosody.syllables.len();
        if !push_variant(
            &mut application,
            variant,
            declaration,
            vec![StructuralEdit::Syllabify { syllables: count }],
            variant_limit,
        ) {
            break;
        }
    }
    Ok(application)
}

fn apply_stress(
    candidate: &StructuredCandidate,
    declaration: &OperationDeclaration,
    specification: &StressAssignmentSpec,
    variant_limit: usize,
) -> Result<OperationApplication, GenerationReason> {
    let count = candidate.surface().prosody.syllables.len();
    if count == 0 {
        return Ok(OperationApplication::empty());
    }
    let positions: Vec<_> = match specification.primary {
        StressPosition::Any => (0..count).collect(),
        StressPosition::Initial => vec![0],
        StressPosition::Final => vec![count - 1],
        StressPosition::Penultimate => vec![count.saturating_sub(2)],
        StressPosition::Index(index) if index < count => vec![index],
        StressPosition::Index(_) => Vec::new(),
    };
    let mut application = OperationApplication::empty();
    for primary in positions {
        let mut variant = candidate.clone();
        for syllable in &mut variant.surface_mut().prosody.syllables {
            syllable.stress = StressLevel::Unstressed;
        }
        variant.surface_mut().prosody.syllables[primary].stress = StressLevel::Primary;
        match specification.secondary {
            SecondaryStressPolicy::None => {}
            SecondaryStressPolicy::AlternatingLeftToRight => {
                for index in (0..count).step_by(2) {
                    if index != primary {
                        variant.surface_mut().prosody.syllables[index].stress =
                            StressLevel::Secondary;
                    }
                }
            }
            SecondaryStressPolicy::AlternatingRightToLeft => {
                let parity = (count - 1) % 2;
                for index in 0..count {
                    if index % 2 == parity && index != primary {
                        variant.surface_mut().prosody.syllables[index].stress =
                            StressLevel::Secondary;
                    }
                }
            }
        }
        let primary_id = variant.surface().prosody.syllables[primary].id;
        if !push_variant(
            &mut application,
            variant,
            declaration,
            vec![StructuralEdit::AssignStress {
                primary: Some(primary_id),
            }],
            variant_limit,
        ) {
            break;
        }
    }
    Ok(application)
}

fn tone_targets(form: &PhonologicalForm, target: ToneTarget) -> Vec<AssociationTarget> {
    match target {
        ToneTarget::Segments => form
            .segments
            .iter()
            .map(|item| AssociationTarget::Segment(item.id))
            .collect(),
        ToneTarget::Syllables => form
            .prosody
            .syllables
            .iter()
            .map(|item| AssociationTarget::Syllable(item.id))
            .collect(),
        ToneTarget::Moras => form
            .prosody
            .moras
            .iter()
            .map(|item| AssociationTarget::Mora(item.id))
            .collect(),
    }
}

fn apply_tone(
    candidate: &StructuredCandidate,
    declaration: &OperationDeclaration,
    specification: &ToneAssignmentSpec,
    variant_limit: usize,
) -> Result<OperationApplication, GenerationReason> {
    let targets = tone_targets(candidate.surface(), specification.targets);
    if targets.is_empty() && specification.pattern != TonePattern::Floating {
        return Ok(OperationApplication::empty());
    }
    let mut assignments = Vec::new();
    match specification.pattern {
        TonePattern::Floating | TonePattern::SpreadSingle => {
            for tone in &specification.inventory {
                assignments.push(vec![tone.clone()]);
                if assignments.len() > variant_limit {
                    break;
                }
            }
        }
        TonePattern::OnePerTarget => enumerate_tone_product(
            &specification.inventory,
            targets.len(),
            &mut Vec::new(),
            &mut assignments,
            variant_limit.saturating_add(1),
        ),
    }
    let mut application = OperationApplication::empty();
    if assignments.len() > variant_limit {
        application.truncated = Some(
            GenerationReason::new(
                GenerationReasonCode::VariantLimit,
                Some(declaration.id.clone()),
                "generator.operations.assign-tone",
                "tone assignments exceeded the per-application budget",
            )
            .unfinished("remaining tone-to-bearing-unit assignments"),
        );
        return Ok(application);
    }
    for values in assignments {
        let mut variant = candidate.clone();
        let nodes: Vec<_> = values
            .iter()
            .enumerate()
            .map(|(index, value)| TierNode {
                id: TierNodeId(index as u32),
                value: TierValue::Tone(value.clone()),
            })
            .collect();
        let associations = match specification.pattern {
            TonePattern::Floating => Vec::new(),
            TonePattern::SpreadSingle => targets
                .iter()
                .cloned()
                .map(|target| TierAssociation {
                    node: TierNodeId(0),
                    target,
                })
                .collect(),
            TonePattern::OnePerTarget => targets
                .iter()
                .cloned()
                .enumerate()
                .map(|(index, target)| TierAssociation {
                    node: TierNodeId(index as u32),
                    target,
                })
                .collect(),
        };
        let tier = AutosegmentalTier {
            name: specification.tier_name.clone(),
            nodes,
            associations,
        };
        let surface = variant.surface_mut();
        surface
            .tiers
            .retain(|item| item.name != specification.tier_name);
        surface.tiers.push(tier);
        surface
            .tiers
            .sort_by(|left, right| left.name.cmp(&right.name));
        if !push_variant(
            &mut application,
            variant,
            declaration,
            vec![StructuralEdit::AssignTone {
                tier: specification.tier_name.clone(),
                nodes: values.len(),
            }],
            variant_limit,
        ) {
            break;
        }
    }
    Ok(application)
}

fn enumerate_usize_product(
    dimensions: &[Vec<usize>],
    depth: usize,
    current: &mut Vec<usize>,
    result: &mut Vec<Vec<usize>>,
    limit: usize,
) {
    if result.len() >= limit {
        return;
    }
    if depth == dimensions.len() {
        result.push(current.clone());
        return;
    }
    for value in &dimensions[depth] {
        current.push(*value);
        enumerate_usize_product(dimensions, depth + 1, current, result, limit);
        current.pop();
        if result.len() >= limit {
            return;
        }
    }
}

fn enumerate_tone_product(
    inventory: &[ToneValue],
    remaining: usize,
    current: &mut Vec<ToneValue>,
    result: &mut Vec<Vec<ToneValue>>,
    limit: usize,
) {
    if result.len() >= limit {
        return;
    }
    if remaining == 0 {
        result.push(current.clone());
        return;
    }
    for tone in inventory {
        current.push(tone.clone());
        enumerate_tone_product(inventory, remaining - 1, current, result, limit);
        current.pop();
        if result.len() >= limit {
            return;
        }
    }
}

fn generation_structure_reason(
    declaration: &OperationDeclaration,
    coordinate: impl Into<String>,
    message: impl Into<String>,
) -> GenerationReason {
    GenerationReason::new(
        GenerationReasonCode::InvalidGeneratedCandidate,
        Some(declaration.id.clone()),
        coordinate,
        message,
    )
}

fn stable_hash(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0100_0000_01b3;
    bytes.iter().fold(OFFSET, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(PRIME)
    })
}

fn search_state_key(candidate: &StructuredCandidate, policy: DeduplicationPolicy) -> Vec<u8> {
    let output_policy = if policy == DeduplicationPolicy::PreserveDerivations {
        DeduplicationPolicy::PreserveDerivations
    } else {
        policy
    };
    let mut key = candidate.canonical_key(output_policy);
    if policy != DeduplicationPolicy::PreserveDerivations {
        let mut uses = BTreeMap::<&str, usize>::new();
        for step in &candidate.derivation {
            *uses.entry(step.operation_id.as_str()).or_default() += 1;
        }
        key.push(0xff);
        key.extend(serde_json::to_vec(&(candidate.derivation.len(), uses)).unwrap_or_default());
    }
    key
}

fn assign_candidate_ids(
    retained: BTreeMap<Vec<u8>, StructuredCandidate>,
    policy: DeduplicationPolicy,
) -> Vec<StructuredCandidate> {
    let mut candidates_by_key = BTreeMap::new();
    for candidate in retained.into_values() {
        candidates_by_key
            .entry(candidate.canonical_key(policy))
            .or_insert(candidate);
    }
    let mut used = BTreeMap::<u64, Vec<u8>>::new();
    candidates_by_key
        .into_iter()
        .map(|(key, mut candidate)| {
            let mut id = stable_hash(&key);
            while used.get(&id).is_some_and(|existing| existing != &key) {
                id = id.wrapping_add(1);
            }
            used.insert(id, key);
            candidate.id = CandidateId(id);
            candidate
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn consonant(symbol: &str) -> SegmentTemplate {
        SegmentTemplate::new(symbol).with_feature("syllabic", FeatureValue::Negative)
    }

    fn vowel(symbol: &str) -> SegmentTemplate {
        SegmentTemplate::new(symbol).with_feature("syllabic", FeatureValue::Positive)
    }

    fn complete_spec(operations: Vec<OperationDeclaration>) -> GeneratorSpec {
        GeneratorSpec {
            name: "test generator".into(),
            operations,
            domain: DeclaredGenerationDomain {
                max_derivation_steps: 2,
                max_segments_per_form: 32,
            },
            resources: GenerationResources {
                max_candidates: 10_000,
                max_operation_expansions: 100_000,
                max_variants_per_application: 10_000,
            },
            support_claim: SupportClaim::CompleteForDeclaredDomain {
                statement: "exhaustive within the declared two-step finite closure".into(),
            },
            deduplication: DeduplicationPolicy::StructuredRepresentation,
        }
    }

    #[test]
    fn identity_candidate_has_explicit_io_correspondence() {
        let input =
            UnderlyingForm::from_segments("kat", [consonant("k"), vowel("a"), consonant("t")]);
        let candidate = StructuredCandidate::identity(&input);
        assert_eq!(candidate.surface_string(), "kat");
        assert!(candidate.validate().is_empty());
        let io = candidate
            .correspondence(&CorrespondenceKind::InputOutput)
            .expect("IO graph");
        assert_eq!(io.links.len(), 3);
        assert!(
            io.links
                .iter()
                .all(|link| link.source.len() == 1 && link.target.len() == 1)
        );
    }

    #[test]
    fn deletion_and_insertion_are_explicit_in_io_graph() {
        let input = UnderlyingForm::from_segments("ab", [vowel("a"), consonant("b")]);
        let specification = complete_spec(vec![
            OperationDeclaration::once(
                "delete-b",
                GenerationOperation::Delete {
                    selector: SegmentSelector::Symbol { symbol: "b".into() },
                },
            ),
            OperationDeclaration::once(
                "insert-t",
                GenerationOperation::Insert {
                    inventory: vec![consonant("t")],
                    sites: InsertionSites::Final,
                },
            ),
        ]);
        let result = FiniteGenerator::generate(&input, &specification);
        assert!(result.status.is_complete());
        let deleted = result
            .candidates
            .iter()
            .find(|candidate| candidate.surface_string() == "a")
            .expect("deletion candidate");
        assert!(
            deleted
                .correspondence(&CorrespondenceKind::InputOutput)
                .unwrap()
                .links
                .iter()
                .any(|link| !link.source.is_empty() && link.target.is_empty())
        );
        let inserted = result
            .candidates
            .iter()
            .find(|candidate| candidate.surface_string() == "abt")
            .expect("insertion candidate");
        assert!(
            inserted
                .correspondence(&CorrespondenceKind::InputOutput)
                .unwrap()
                .links
                .iter()
                .any(|link| link.source.is_empty() && !link.target.is_empty())
        );
    }

    #[test]
    fn generation_is_byte_deterministic_and_deduplicated() {
        let input =
            UnderlyingForm::from_segments("abc", [vowel("a"), consonant("b"), consonant("c")]);
        let mut deletion = OperationDeclaration::once(
            "delete",
            GenerationOperation::Delete {
                selector: SegmentSelector::All,
            },
        );
        deletion.max_applications_per_candidate = 2;
        let spec = complete_spec(vec![deletion]);
        let first = FiniteGenerator::generate(&input, &spec);
        let second = FiniteGenerator::generate(&input, &spec);
        assert_eq!(
            serde_json::to_vec(&first).unwrap(),
            serde_json::to_vec(&second).unwrap()
        );
        let forms: BTreeSet<_> = first
            .candidates
            .iter()
            .map(StructuredCandidate::surface_string)
            .collect();
        assert_eq!(forms.len(), first.candidates.len());
        assert!(first.statistics.duplicates_removed > 0);
    }

    #[test]
    fn candidate_budget_returns_truncated_not_false_completeness() {
        let input =
            UnderlyingForm::from_segments("abc", [vowel("a"), consonant("b"), consonant("c")]);
        let mut spec = complete_spec(vec![OperationDeclaration::once(
            "delete",
            GenerationOperation::Delete {
                selector: SegmentSelector::All,
            },
        )]);
        spec.resources.max_candidates = 2;
        let result = FiniteGenerator::generate(&input, &spec);
        assert!(matches!(
            result.status,
            CompletenessStatus::Truncated { .. }
        ));
        assert!(result.require_complete().is_err());
    }

    #[test]
    fn exploratory_support_never_becomes_complete_by_exhaustion() {
        let input = UnderlyingForm::from_segments("a", [vowel("a")]);
        let mut spec = complete_spec(vec![]);
        spec.support_claim = SupportClaim::Exploratory;
        let result = FiniteGenerator::generate(&input, &spec);
        assert!(matches!(
            result.status,
            CompletenessStatus::Truncated { ref reasons }
                if reasons[0].code == GenerationReasonCode::ExploratorySupport
        ));
    }

    #[test]
    fn invalid_empty_inventory_is_a_structured_refusal() {
        let input = UnderlyingForm::from_segments("a", [vowel("a")]);
        let spec = complete_spec(vec![OperationDeclaration::once(
            "insert",
            GenerationOperation::Insert {
                inventory: vec![],
                sites: InsertionSites::EveryBoundary,
            },
        )]);
        let result = FiniteGenerator::generate(&input, &spec);
        assert!(matches!(
            result.status,
            CompletenessStatus::Refused { ref reasons }
                if reasons.iter().any(|reason| reason.code == GenerationReasonCode::EmptyInventory)
        ));
        assert!(result.candidates.is_empty());
    }

    #[test]
    fn reduplication_constructs_distinct_br_correspondence() {
        let input = UnderlyingForm::from_segments("pa", [consonant("p"), vowel("a")]);
        let spec = complete_spec(vec![OperationDeclaration::once(
            "total-red",
            GenerationOperation::Reduplicate {
                domain: ReduplicationDomain::WholeForm,
                site: ReduplicationSite::Prefix,
            },
        )]);
        let result = FiniteGenerator::generate(&input, &spec);
        let reduplicated = result
            .candidates
            .iter()
            .find(|candidate| candidate.surface_string() == "papa")
            .expect("total reduplication candidate");
        assert!(
            reduplicated
                .correspondence(&CorrespondenceKind::BaseReduplicant)
                .is_some()
        );
        assert!(reduplicated.validate().is_empty());
    }

    #[test]
    fn feature_change_metathesis_and_affixation_are_general_operations() {
        let input =
            UnderlyingForm::from_segments("pat", [consonant("p"), vowel("a"), consonant("t")]);
        let spec = GeneratorSpec {
            domain: DeclaredGenerationDomain {
                max_derivation_steps: 1,
                max_segments_per_form: 16,
            },
            operations: vec![
                OperationDeclaration::once(
                    "voice",
                    GenerationOperation::FeatureChange {
                        selector: SegmentSelector::Symbol { symbol: "t".into() },
                        feature: FeatureName("voice".into()),
                        values: vec![FeatureValue::Positive],
                    },
                ),
                OperationDeclaration::once(
                    "transpose",
                    GenerationOperation::Metathesis {
                        selector: SegmentSelector::All,
                        max_distance: 2,
                    },
                ),
                OperationDeclaration::once(
                    "suffix",
                    GenerationOperation::Affix {
                        morpheme: MorphemeTemplate {
                            label: "PL".into(),
                            kind: MorphemeKind::Suffix,
                            segments: vec![consonant("s")],
                        },
                        site: AffixSite::Suffix,
                    },
                ),
            ],
            ..complete_spec(vec![])
        };
        let result = FiniteGenerator::generate(&input, &spec);
        assert!(result.status.is_complete());
        assert!(result.candidates.iter().any(|candidate| {
            candidate.surface().segments.iter().any(|segment| {
                segment.symbol == "t"
                    && segment.features.get(&FeatureName("voice".into()))
                        == Some(&FeatureValue::Positive)
            })
        }));
        assert!(
            result
                .candidates
                .iter()
                .any(|candidate| candidate.surface_string() == "tap")
        );
        let affixed = result
            .candidates
            .iter()
            .find(|candidate| candidate.surface_string() == "pats")
            .expect("suffix candidate");
        assert!(
            affixed
                .surface()
                .morphemes
                .iter()
                .any(|morpheme| morpheme.label == "PL")
        );
    }

    #[test]
    fn many_to_many_graphs_encode_fusion_and_split_without_special_flags() {
        let input = UnderlyingForm::from_segments("ab", [vowel("a"), consonant("b")]);
        let mut candidate = StructuredCandidate::identity(&input);
        let graph = CorrespondenceGraph {
            id: CorrespondenceGraphId(99),
            label: "declared fusion and split".into(),
            kind: CorrespondenceKind::UserNamed("analysis-specific".into()),
            source_form: candidate.underlying_form,
            target_form: candidate.surface_form,
            links: vec![
                CorrespondenceLink {
                    id: CorrespondenceLinkId(0),
                    source: vec![
                        CorrespondenceNode::Segment(SegmentId(0)),
                        CorrespondenceNode::Segment(SegmentId(1)),
                    ],
                    target: vec![CorrespondenceNode::Segment(SegmentId(0))],
                },
                CorrespondenceLink {
                    id: CorrespondenceLinkId(1),
                    source: vec![CorrespondenceNode::Segment(SegmentId(1))],
                    target: vec![
                        CorrespondenceNode::Segment(SegmentId(0)),
                        CorrespondenceNode::Segment(SegmentId(1)),
                    ],
                },
            ],
        };
        candidate.add_correspondence(graph).unwrap();
        assert!(candidate.validate().is_empty());
        let graph = candidate
            .correspondence(&CorrespondenceKind::UserNamed("analysis-specific".into()))
            .unwrap();
        assert!(graph.links.iter().any(|link| link.source.len() == 2));
        assert!(graph.links.iter().any(|link| link.target.len() == 2));
    }

    #[test]
    fn internal_id_renaming_does_not_duplicate_one_structured_candidate() {
        let input = UnderlyingForm::from_segments("a", [vowel("a")]);
        let spec = GeneratorSpec {
            domain: DeclaredGenerationDomain {
                max_derivation_steps: 2,
                max_segments_per_form: 8,
            },
            operations: vec![
                OperationDeclaration::once(
                    "insert-p",
                    GenerationOperation::Insert {
                        inventory: vec![consonant("p")],
                        sites: InsertionSites::Before {
                            selector: SegmentSelector::Or {
                                selectors: vec![
                                    SegmentSelector::Symbol { symbol: "a".into() },
                                    SegmentSelector::Symbol { symbol: "t".into() },
                                ],
                            },
                        },
                    },
                ),
                OperationDeclaration::once(
                    "insert-t",
                    GenerationOperation::Insert {
                        inventory: vec![consonant("t")],
                        sites: InsertionSites::Before {
                            selector: SegmentSelector::Symbol { symbol: "a".into() },
                        },
                    },
                ),
            ],
            ..complete_spec(vec![])
        };
        let result = FiniteGenerator::generate(&input, &spec);
        assert!(result.status.is_complete());
        assert_eq!(
            result
                .candidates
                .iter()
                .filter(|candidate| candidate.surface_string() == "pta")
                .count(),
            1
        );
    }

    #[test]
    fn tone_cartesian_product_obeys_variant_guardrail() {
        let input = UnderlyingForm::from_segments("aaa", [vowel("a"), vowel("a"), vowel("a")]);
        let mut candidate = StructuredCandidate::identity(&input);
        candidate.surface_mut().prosody.syllables = (0..3)
            .map(|index| Syllable {
                id: SyllableId(index),
                onset: vec![],
                nucleus: vec![SegmentId(index)],
                coda: vec![],
                stress: StressLevel::Unstressed,
            })
            .collect();
        let declaration = OperationDeclaration::once(
            "tone",
            GenerationOperation::AssignTone {
                specification: ToneAssignmentSpec {
                    tier_name: "tone".into(),
                    inventory: vec![ToneValue::Level(1), ToneValue::Level(5)],
                    targets: ToneTarget::Syllables,
                    pattern: TonePattern::OnePerTarget,
                },
            },
        );
        let application = apply_operation(
            &candidate,
            &declaration,
            &DeclaredGenerationDomain::default(),
            7,
        )
        .unwrap();
        assert!(application.candidates.is_empty());
        assert!(matches!(
            application.truncated,
            Some(GenerationReason {
                code: GenerationReasonCode::VariantLimit,
                ..
            })
        ));
    }

    #[test]
    fn user_can_add_oo_and_sympathy_graphs_without_retyping_io() {
        let input = UnderlyingForm::from_segments("a", [vowel("a")]);
        let mut candidate = StructuredCandidate::identity(&input);
        let mut related = candidate.surface().clone();
        related.role = FormRole::RelatedSurface {
            relation: "paradigm-base".into(),
        };
        let related_id = candidate.add_related_form(related).unwrap();
        let graph = CorrespondenceGraph::identity_segments(
            CorrespondenceGraphId(99),
            "OO",
            CorrespondenceKind::OutputOutput,
            candidate.surface(),
            &candidate.forms[&related_id],
        );
        candidate.add_correspondence(graph).unwrap();
        let mut sympathetic = candidate.surface().clone();
        sympathetic.role = FormRole::Sympathetic;
        let sympathetic_id = candidate.add_related_form(sympathetic).unwrap();
        let graph = CorrespondenceGraph::identity_segments(
            CorrespondenceGraphId(100),
            "Sympathy",
            CorrespondenceKind::Sympathy,
            &candidate.forms[&sympathetic_id],
            candidate.surface(),
        );
        candidate.add_correspondence(graph).unwrap();
        assert!(
            candidate
                .correspondence(&CorrespondenceKind::OutputOutput)
                .is_some()
        );
        assert!(
            candidate
                .correspondence(&CorrespondenceKind::Sympathy)
                .is_some()
        );
        assert_eq!(
            candidate
                .correspondences_of_kind(CorrespondenceKind::InputOutput)
                .count(),
            1
        );
    }

    #[test]
    fn syllabification_stress_and_tone_are_structural_operations() {
        let input = UnderlyingForm::from_segments(
            "pata",
            [consonant("p"), vowel("a"), consonant("t"), vowel("a")],
        );
        let spec = GeneratorSpec {
            domain: DeclaredGenerationDomain {
                max_derivation_steps: 3,
                max_segments_per_form: 16,
            },
            operations: vec![
                OperationDeclaration::once(
                    "syllabify",
                    GenerationOperation::Syllabify {
                        specification: SyllabificationSpec {
                            nucleus_selector: SegmentSelector::Feature {
                                name: FeatureName("syllabic".into()),
                                value: FeatureValue::Positive,
                            },
                            max_onset: 1,
                            max_coda: 1,
                            allow_empty_onset: true,
                            allow_empty_coda: true,
                        },
                    },
                ),
                OperationDeclaration::once(
                    "stress",
                    GenerationOperation::AssignStress {
                        specification: StressAssignmentSpec {
                            primary: StressPosition::Any,
                            secondary: SecondaryStressPolicy::None,
                        },
                    },
                ),
                OperationDeclaration::once(
                    "tone",
                    GenerationOperation::AssignTone {
                        specification: ToneAssignmentSpec {
                            tier_name: "tone".into(),
                            inventory: vec![ToneValue::Level(1), ToneValue::Level(5)],
                            targets: ToneTarget::Syllables,
                            pattern: TonePattern::SpreadSingle,
                        },
                    },
                ),
            ],
            ..complete_spec(vec![])
        };
        let result = FiniteGenerator::generate(&input, &spec);
        assert!(result.candidates.iter().any(|candidate| {
            candidate.surface().prosody.syllables.len() == 2
                && candidate
                    .surface()
                    .prosody
                    .syllables
                    .iter()
                    .filter(|syllable| syllable.stress == StressLevel::Primary)
                    .count()
                    == 1
                && candidate
                    .surface()
                    .tiers
                    .iter()
                    .any(|tier| tier.name == "tone")
        }));
    }

    #[test]
    fn flat_projection_preserves_candidate_not_output_terminology() {
        let input = UnderlyingForm::from_segments("ta", [consonant("t"), vowel("a")]);
        let candidate = StructuredCandidate::identity(&input);
        let flat = candidate.to_flat_candidate(vec![0, 1]).unwrap();
        assert_eq!(flat.form, "ta");
        assert_eq!(flat.violations, vec![0, 1]);
        assert!(flat.base_mass.is_exact());
        assert!(flat.observed_frequency.is_exact());
    }

    #[test]
    fn combinatorial_guardrail_bounds_two_deletions() {
        let symbols = (0..12).map(|index| consonant(&format!("C{index}")));
        let input = UnderlyingForm::from_segments("twelve", symbols);
        let mut deletion = OperationDeclaration::once(
            "delete",
            GenerationOperation::Delete {
                selector: SegmentSelector::All,
            },
        );
        deletion.max_applications_per_candidate = 2;
        let result = FiniteGenerator::generate(&input, &complete_spec(vec![deletion]));
        // 1 identity + C(12,1) + C(12,2); path duplicates must not survive.
        assert_eq!(result.candidates.len(), 79);
        assert!(result.statistics.operation_expansions <= 13);
        assert!(result.statistics.duplicates_removed >= 66);
    }

    proptest! {
        #[test]
        fn one_deletion_removes_exactly_one_segment(length in 1usize..24) {
            let input = UnderlyingForm::from_segments(
                "input",
                (0..length).map(|index| consonant(&format!("s{index}"))),
            );
            let mut spec = complete_spec(vec![OperationDeclaration::once(
                "delete",
                GenerationOperation::Delete { selector: SegmentSelector::All },
            )]);
            spec.domain.max_derivation_steps = 1;
            let result = FiniteGenerator::generate(&input, &spec);
            prop_assert!(result.status.is_complete());
            prop_assert_eq!(result.candidates.len(), length + 1);
            let every_candidate_has_expected_length = result.candidates.iter().all(|candidate| {
                candidate.surface().segments.len() == length
                    || candidate.surface().segments.len() + 1 == length
            });
            prop_assert!(every_candidate_has_expected_length);
        }

        #[test]
        fn serde_round_trip_preserves_structured_candidate(length in 0usize..20) {
            let input = UnderlyingForm::from_segments(
                "input",
                (0..length).map(|index| consonant(&format!("s{index}"))),
            );
            let candidate = StructuredCandidate::identity(&input);
            let bytes = serde_json::to_vec(&candidate).unwrap();
            let decoded: StructuredCandidate = serde_json::from_slice(&bytes).unwrap();
            prop_assert_eq!(candidate, decoded);
        }
    }
}
