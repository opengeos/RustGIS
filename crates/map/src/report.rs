//! Reports: a layer's records laid out page after page, grouped, summarised with statistics and
//! charts, with pictures and attachments, related records (subreports), extra pages and other
//! reports appended. A [`Report`] says what goes in; the engine turns it into a [`Doc`] (a flat
//! list of blocks) and the renderer paginates that.

use serde::{Deserialize, Serialize};

/// Most fields, groups, statistics and sections a report keeps (files are hostile).
pub const MAX_ITEMS: usize = 64;
/// Most group levels.
pub const MAX_GROUPS: usize = 4;
/// Page sizes are clamped to this range, in millimetres.
pub const MIN_PAGE: f64 = 50.0;
pub const MAX_PAGE: f64 = 2000.0;

/// A report on one layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Report {
    pub name: String,
    pub title: String,
    /// The layer reported on, by name (empty in a report template).
    pub layer: String,
    /// Only records matching this where clause (empty: all).
    #[serde(rename = "where")]
    pub where_clause: String,
    /// Only the layer's selected records.
    pub selected_only: bool,
    /// Page size in millimetres.
    pub width: f64,
    pub height: f64,
    /// The detail fields, one column each.
    pub fields: Vec<String>,
    /// Group fields, outermost first: each starts a heading when its value changes.
    pub groups: Vec<String>,
    /// Records sort by this field within their group.
    pub sort: Option<String>,
    pub descending: bool,
    /// Statistics shown at the end of each group and of the report.
    pub stats: Vec<Stat>,
    /// A field holding a picture per record (a PNG/JPEG path, or base64).
    pub picture_field: Option<String>,
    /// A field holding each record's attachments (paths or names, separated by `;`).
    pub attachments_field: Option<String>,
    /// A chart at the end of the report.
    pub chart: Option<ReportChart>,
    /// Extra pages, related reports and subreports.
    pub sections: Vec<Section>,
}

impl Default for Report {
    fn default() -> Self {
        Report {
            name: "Report".into(),
            title: String::new(),
            layer: String::new(),
            where_clause: String::new(),
            selected_only: false,
            width: 215.9,
            height: 279.4,
            fields: Vec::new(),
            groups: Vec::new(),
            sort: None,
            descending: false,
            stats: Vec::new(),
            picture_field: None,
            attachments_field: None,
            chart: None,
            sections: Vec::new(),
        }
    }
}

impl Report {
    /// Makes a report read from a file safe to use: bounded lists, a sane page size.
    pub fn clamp(&mut self) {
        let page = |v: f64, d: f64| if v.is_finite() { v.clamp(MIN_PAGE, MAX_PAGE) } else { d };
        self.width = page(self.width, 215.9);
        self.height = page(self.height, 279.4);
        self.name = self.name.chars().take(200).collect();
        self.title = self.title.chars().take(500).collect();
        self.fields.truncate(MAX_ITEMS);
        self.groups.truncate(MAX_GROUPS);
        self.stats.truncate(MAX_ITEMS);
        self.sections.truncate(MAX_ITEMS);
        for s in &mut self.sections {
            if let Section::Subreport { fields, .. } = s {
                fields.truncate(MAX_ITEMS);
            }
        }
    }

    /// The report as a template: its design without its data source or links to other items.
    pub fn template(&self) -> Report {
        let mut t = self.clone();
        t.layer.clear();
        t.where_clause.clear();
        t.selected_only = false;
        t.sections.retain(|s| matches!(s, Section::Page { .. }));
        t
    }
}

/// A summary statistic of a field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stat {
    pub field: String,
    pub kind: StatKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatKind {
    Count,
    Sum,
    Mean,
    Min,
    Max,
}

impl StatKind {
    pub fn from_code(s: &str) -> Option<StatKind> {
        Some(match s.to_ascii_lowercase().as_str() {
            "count" => StatKind::Count,
            "sum" => StatKind::Sum,
            "mean" | "average" => StatKind::Mean,
            "min" | "minimum" => StatKind::Min,
            "max" | "maximum" => StatKind::Max,
            _ => return None,
        })
    }
    pub fn label(self) -> &'static str {
        match self {
            StatKind::Count => "Count",
            StatKind::Sum => "Sum",
            StatKind::Mean => "Mean",
            StatKind::Min => "Minimum",
            StatKind::Max => "Maximum",
        }
    }
    /// The statistic of `values` (`count` counts the records, numbers or not).
    pub fn apply(self, values: &[f64], records: usize) -> Option<f64> {
        let n = values.len() as f64;
        match self {
            StatKind::Count => Some(records as f64),
            _ if values.is_empty() => None,
            StatKind::Sum => Some(values.iter().sum()),
            StatKind::Mean => Some(values.iter().sum::<f64>() / n),
            StatKind::Min => values.iter().copied().reduce(f64::min),
            StatKind::Max => values.iter().copied().reduce(f64::max),
        }
    }
}

/// A bar chart: the records per category, or the sum of a value field per category.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReportChart {
    pub category: String,
    #[serde(default)]
    pub value: Option<String>,
}

/// A part of the report beyond its records.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Section {
    /// A page of text, before the records or after them.
    Page {
        title: String,
        text: String,
        #[serde(default)]
        end: bool,
    },
    /// A page layout drawn as a page of the report.
    Layout {
        layout: String,
        #[serde(default)]
        end: bool,
    },
    /// Another report appended after this one.
    Related { report: String },
    /// Under each record, its related records through a relationship.
    Subreport {
        relationship: String,
        #[serde(default)]
        fields: Vec<String>,
    },
}

/// A report ready to paginate: page size, title and blocks, top to bottom.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Doc {
    pub title: String,
    pub width: f64,
    pub height: f64,
    pub blocks: Vec<Block>,
}

/// One piece of a report.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    /// The report's title, large.
    Title(String),
    /// A group heading (level 0 outermost) or section heading.
    Heading { level: u8, text: String },
    /// Column names for the rows below (repeated at the top of each page).
    Columns(Vec<String>),
    /// One record.
    Row(Vec<String>),
    /// Column names of related records, indented.
    SubColumns(Vec<String>),
    /// One related record, indented.
    SubRow(Vec<String>),
    /// Summary statistics, one per line.
    Stats(Vec<String>),
    /// A record's picture (base64 PNG/JPEG) and its pixel size.
    Picture { image: String, width: u32, height: u32 },
    /// A record's attachments, by name.
    Attachments(Vec<String>),
    /// A bar chart.
    Chart { title: String, bars: Vec<(String, f64)> },
    /// Paragraphs of text, wrapped to the page.
    Text(String),
    /// Starts a new page.
    PageBreak,
    /// A page layout (the project's layout at this index) as a whole page.
    Layout(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_templates_and_clamping() {
        assert_eq!(StatKind::Mean.apply(&[1.0, 2.0, 6.0], 3), Some(3.0));
        assert_eq!(StatKind::Count.apply(&[], 5), Some(5.0));
        assert_eq!(StatKind::Max.apply(&[], 5), None);
        let mut r = Report {
            layer: "Cities".into(),
            where_clause: "pop > 1".into(),
            width: f64::NAN,
            height: 1e9,
            groups: vec!["a".into(); 10],
            sections: vec![
                Section::Page { title: "Intro".into(), text: String::new(), end: false },
                Section::Related { report: "Other".into() },
            ],
            ..Report::default()
        };
        r.clamp();
        assert_eq!((r.width, r.height, r.groups.len()), (215.9, MAX_PAGE, MAX_GROUPS));
        let t = r.template();
        assert!(t.layer.is_empty() && t.where_clause.is_empty() && t.sections.len() == 1);
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<Report>(&json).unwrap(), r);
    }
}
