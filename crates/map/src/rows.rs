//! Layer policies around the data crate's definition-query matcher.

use crate::Layer;
use rustgis_data::Dataset;

/// Which layer restrictions a consumer honors. Whole-layer tools deliberately use `All`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    All,
    Query,
    SelectionAndQuery,
}

/// An empty subset means no rows, never every row. Legacy analysis adapters must refuse an
/// empty subset before handing a list to tools whose empty-list convention means all rows.
#[derive(Debug, PartialEq, Eq)]
pub enum Rows {
    All,
    Subset(Vec<usize>),
}

impl Layer {
    /// Resolve against the consumed dataset, not a streamed layer's resident window. `within`
    /// is an optional extent restriction, used only without a selection. Invalid queries leave
    /// rows unfiltered, as on the map; consumers needing a notice use `data::rows::Query` directly.
    /// Query evaluation is one pass without Cancel, like `query_rows`; traversal stays the caller's.
    pub fn resolve_rows(&self, dataset: &Dataset, policy: Policy, within: Option<&[usize]>) -> Result<Rows, String> {
        if policy == Policy::All {
            return Ok(Rows::All);
        }
        let current = if policy == Policy::SelectionAndQuery && !self.selection.is_empty() {
            Some(self.selected_rows_in(dataset)?)
        } else {
            within.map(<[usize]>::to_vec)
        };
        let Some(keep) = self.query_rows(dataset) else {
            return Ok(current.map_or(Rows::All, Rows::Subset));
        };
        let kept = |row: &usize| keep.get(*row).is_some_and(|keep| *keep);
        Ok(Rows::Subset(match current {
            Some(rows) => rows.into_iter().filter(kept).collect(),
            None => (0..keep.len()).filter(kept).collect(),
        }))
    }

    /// Map window selections by source feature ID when consuming another dataset. Selection
    /// order is retained, and a disappeared feature is an error rather than silently dropped.
    fn selected_rows_in(&self, dataset: &Dataset) -> Result<Vec<usize>, String> {
        if std::ptr::eq(dataset, &self.dataset) {
            return Ok(self.selection.clone());
        }
        let missing = || format!("{}: a selected feature is no longer in the source", self.name);
        let mut by_id = self
            .selection
            .iter()
            .map(|row| self.dataset.features.get(*row).map(|f| (f.id, None)).ok_or_else(missing))
            .collect::<Result<std::collections::HashMap<u64, Option<usize>>, _>>()?;
        for (row, feature) in dataset.features.iter().enumerate() {
            if let Some(selected) = by_id.get_mut(&feature.id) {
                *selected = Some(row);
            }
        }
        self.selection
            .iter()
            .map(|row| self.dataset.features.get(*row).and_then(|f| by_id.get(&f.id)).copied().flatten().ok_or_else(missing))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgis_data::{Field, FieldType, GeometryType, Value};

    #[test]
    fn row_policy_matrix_keeps_empty_subsets_and_source_identity() {
        let mut ds = Dataset::new("rows", GeometryType::Point, vec![Field::new("n", FieldType::Integer)]);
        for n in 1..=3 {
            ds.push(None, vec![Value::Int(n)]);
        }
        let mut layer = Layer::new(1, ds);
        for (query, selection, within, expected) in [
            (None, vec![], None, Rows::All),
            (Some("n >= 2"), vec![], None, Rows::Subset(vec![1, 2])),
            (None, vec![2, 0], None, Rows::Subset(vec![2, 0])),
            (Some("n >= 2"), vec![0, 2], None, Rows::Subset(vec![2])),
            (Some("n > 3"), vec![], None, Rows::Subset(vec![])),
            (Some("n >= 2"), vec![0], None, Rows::Subset(vec![])),
            (None, vec![], Some(vec![]), Rows::Subset(vec![])),
            (Some("n >= 2"), vec![], Some(vec![0, 1]), Rows::Subset(vec![1])),
            (Some("n >= 2"), vec![2], Some(vec![]), Rows::Subset(vec![2])),
            (Some("unknown = 1"), vec![2], None, Rows::Subset(vec![2])),
            (Some("n = = 1"), vec![], None, Rows::All),
            (Some("   "), vec![], None, Rows::All),
        ] {
            layer.definition_query = query.map(str::to_string);
            layer.selection = selection;
            assert_eq!(layer.resolve_rows(&layer.dataset, Policy::SelectionAndQuery, within.as_deref()).unwrap(), expected);
        }
        layer.selection = vec![0];
        layer.definition_query = Some("n = 2".into());
        assert_eq!(layer.resolve_rows(&layer.dataset, Policy::Query, None).unwrap(), Rows::Subset(vec![1]));
        assert_eq!(layer.resolve_rows(&layer.dataset, Policy::All, Some(&[])).unwrap(), Rows::All);
        let mut full = layer.dataset.clone();
        // A streamed window's first row is the full source's last: position must not select n=1.
        layer.dataset.features = vec![full.features[2].clone()].into();
        layer.definition_query = Some("n = 3".into());
        assert_eq!(layer.resolve_rows(&full, Policy::SelectionAndQuery, None).unwrap(), Rows::Subset(vec![2]));
        full.features.pop();
        assert!(layer.resolve_rows(&full, Policy::SelectionAndQuery, None).unwrap_err().contains("no longer in the source"));
    }
}
