use serde::{Deserialize, Serialize};

use crate::ax_ast::prelude::AxExpr;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxQuerySpec {
    pub source: AxQuerySource,
    pub joins: Vec<AxQueryJoin>,
    pub filters: Vec<AxQueryFilter>,
    pub orders: Vec<AxQueryOrder>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub mode: AxQueryMode,
}

impl AxQuerySpec {
    pub fn new(source: AxQuerySource) -> Self {
        Self {
            source,
            joins: Vec::new(),
            filters: Vec::new(),
            orders: Vec::new(),
            limit: None,
            offset: None,
            mode: AxQueryMode::Many,
        }
    }

    pub fn join(mut self, join: AxQueryJoin) -> Self {
        self.joins.push(join);
        self
    }

    pub fn filter(mut self, filter: AxQueryFilter) -> Self {
        self.filters.push(filter);
        self
    }

    pub fn order(mut self, order: AxQueryOrder) -> Self {
        self.orders.push(order);
        self
    }

    pub fn limit(mut self, value: u32) -> Self {
        self.limit = Some(value);
        self
    }

    pub fn offset(mut self, value: u32) -> Self {
        self.offset = Some(value);
        self
    }

    pub fn first(mut self) -> Self {
        self.mode = AxQueryMode::First;
        self.limit.get_or_insert(1);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxQueryJoin {
    pub collection: String,
    pub columns: Vec<AxQueryJoinColumn>,
}

impl AxQueryJoin {
    pub fn new(
        collection: impl Into<String>,
        columns: impl IntoIterator<Item = AxQueryJoinColumn>,
    ) -> Self {
        Self {
            collection: collection.into(),
            columns: columns.into_iter().collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxQueryJoinColumn {
    pub source: String,
    pub target: String,
}

impl AxQueryJoinColumn {
    pub fn new(source: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AxQuerySource {
    Stream { collection: String },
    ContentCollection { collection: String },
    RawSql { sql: String, params: Vec<AxExpr> },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AxQueryMode {
    Many,
    First,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxQueryFilter {
    pub field: String,
    pub op: AxQueryFilterOp,
    pub value: AxExpr,
}

impl AxQueryFilter {
    pub fn new(field: impl Into<String>, op: AxQueryFilterOp, value: impl Into<AxExpr>) -> Self {
        Self {
            field: field.into(),
            op,
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AxQueryFilterOp {
    Eq,
    Ne,
    In,
    NotIn,
    IsNull,
    IsNotNull,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxQueryOrder {
    pub field: String,
    pub direction: AxQueryOrderDirection,
}

impl AxQueryOrder {
    pub fn new(field: impl Into<String>, direction: AxQueryOrderDirection) -> Self {
        Self {
            field: field.into(),
            direction,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AxQueryOrderDirection {
    Asc,
    Desc,
}

pub mod prelude {
    pub use super::AxQueryFilter;
    pub use super::AxQueryFilterOp;
    pub use super::AxQueryJoin;
    pub use super::AxQueryJoinColumn;
    pub use super::AxQueryMode;
    pub use super::AxQueryOrder;
    pub use super::AxQueryOrderDirection;
    pub use super::AxQuerySource;
    pub use super::AxQuerySpec;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_spec_can_model_filters_sorting_and_paging() {
        let query = AxQuerySpec::new(AxQuerySource::Stream {
            collection: "posts".to_string(),
        })
        .filter(AxQueryFilter::new(
            "status",
            AxQueryFilterOp::Eq,
            AxExpr::string("published"),
        ))
        .order(AxQueryOrder::new("created_at", AxQueryOrderDirection::Desc))
        .limit(20)
        .offset(40);

        assert_eq!(
            query,
            AxQuerySpec {
                source: AxQuerySource::Stream {
                    collection: "posts".to_string(),
                },
                joins: vec![],
                filters: vec![AxQueryFilter {
                    field: "status".to_string(),
                    op: AxQueryFilterOp::Eq,
                    value: AxExpr::String("published".to_string()),
                }],
                orders: vec![AxQueryOrder {
                    field: "created_at".to_string(),
                    direction: AxQueryOrderDirection::Desc,
                }],
                limit: Some(20),
                offset: Some(40),
                mode: AxQueryMode::Many,
            }
        );
    }

    #[test]
    fn query_spec_can_model_content_collections() {
        let query = AxQuerySpec::new(AxQuerySource::ContentCollection {
            collection: "docs".to_string(),
        })
        .order(AxQueryOrder::new("slug", AxQueryOrderDirection::Asc));

        assert_eq!(
            query.source,
            AxQuerySource::ContentCollection {
                collection: "docs".to_string()
            }
        );
        assert_eq!(query.orders.len(), 1);
    }

    #[test]
    fn query_spec_can_model_first_result_mode() {
        let query = AxQuerySpec::new(AxQuerySource::Stream {
            collection: "posts".to_string(),
        })
        .first();

        assert_eq!(query.mode, AxQueryMode::First);
        assert_eq!(query.limit, Some(1));
    }

    #[test]
    fn query_spec_can_model_raw_sql_escape_hatch() {
        let query = AxQuerySpec::new(AxQuerySource::RawSql {
            sql: "select * from posts where status = ?".to_string(),
            params: vec![AxExpr::string("published")],
        });

        assert_eq!(
            query.source,
            AxQuerySource::RawSql {
                sql: "select * from posts where status = ?".to_string(),
                params: vec![AxExpr::String("published".to_string())],
            }
        );
    }
}
