//! Shared metadata for field/statistic editors; command values retain their JSON contracts.
use rustgis_data::FieldType;

/// The vocabulary and object keys of an existing field/statistic list parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatisticList {
    /// Summary Statistics objects with `field` and `statistic` keys; fields may repeat.
    Summary,
    /// Spatial Join objects with `field`, `rule` and an optional `delimiter`; one rule per field.
    Merge,
}

impl StatisticList {
    /// Key paired with `field` in each JSON object.
    pub fn key(self) -> &'static str {
        match self {
            Self::Summary => "statistic",
            Self::Merge => "rule",
        }
    }

    /// Recognizes the same spellings as execution and returns their canonical identity, without
    /// changing saved values. For example, `P01` and `P1` name the same summary statistic.
    pub fn canonical_name(self, name: &str) -> Option<String> {
        match self {
            Self::Summary => crate::stats::Stat::parse(name).map(|stat| stat.name()),
            Self::Merge => {
                crate::join::RULES.iter().find(|(rule, _)| rule.eq_ignore_ascii_case(name.trim())).map(|(rule, _)| (*rule).to_string())
            }
        }
    }

    /// Supported names, filtered using the same type rules as execution. Unknown input types
    /// offer all choices so a model can be configured before its upstream tool has run.
    pub fn choices(self, ty: Option<FieldType>) -> Vec<String> {
        match self {
            Self::Summary => crate::stats::NAMED
                .iter()
                .filter(|(_, stat)| ty.is_none_or(|t| t.is_numeric() || stat.fits_text()))
                .map(|(name, _)| (*name).to_string())
                .chain((1..=99).filter(|_| ty.is_none_or(|t| t.is_numeric())).map(|k| format!("P{k}")))
                .collect(),
            Self::Merge => {
                crate::join::RULES.iter().filter(|(_, rule)| ty.is_none_or(|t| rule.fits(t))).map(|(name, _)| (*name).to_string()).collect()
            }
        }
    }
}
