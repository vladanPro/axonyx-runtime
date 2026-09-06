use thiserror::Error;

use crate::ax_backend_lowering::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxSqlDialect {
    Postgres,
    MySql,
    Sqlite,
}

impl AxSqlDialect {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::MySql => "mysql",
            Self::Sqlite => "sqlite",
        }
    }

    fn quote_ident(&self, ident: &str) -> String {
        match self {
            Self::Postgres | Self::Sqlite => format!("\"{ident}\""),
            Self::MySql => format!("`{ident}`"),
        }
    }

    fn placeholder(&self, index: usize) -> String {
        match self {
            Self::Postgres => format!("${index}"),
            Self::MySql | Self::Sqlite => "?".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxSqlQuery {
    pub sql: String,
    pub params: Vec<AxSqlParam>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxSqlMutation {
    pub sql: String,
    pub params: Vec<AxSqlParam>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxSqlParam {
    pub index: usize,
    pub value: AxRustExpr,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AxSqlCompileError {
    #[error("query collection cannot be empty")]
    EmptyCollection,
    #[error("raw SQL queries are executed by the runtime escape hatch")]
    RawSqlRuntimeOnly,
    #[error("identifier `{ident}` contains unsupported characters")]
    InvalidIdentifier { ident: String },
    #[error("join target `{collection}` is duplicated or aliases the source collection")]
    AmbiguousJoinCollection { collection: String },
    #[error("join target `{collection}` must map at least one column")]
    EmptyJoinColumns { collection: String },
    #[error("join target `{collection}` maps duplicate source or target columns")]
    DuplicateJoinColumns { collection: String },
    #[error("query field `{field}` uses unknown collection qualifier `{qualifier}`")]
    UnknownQueryQualifier { field: String, qualifier: String },
    #[error("unsupported query filter operator")]
    UnsupportedFilterOperator,
    #[error("mutation must contain at least one field")]
    EmptyMutationFields,
    #[error("delete mutation must contain at least one filter")]
    EmptyDeleteFilters,
    #[error("list filters require a literal list value")]
    NonLiteralListFilter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AxSqlFilterContext {
    Query,
    Mutation,
}

pub fn compile_query_plan_to_sql(
    query: &AxQueryPlan,
    dialect: AxSqlDialect,
) -> Result<AxSqlQuery, AxSqlCompileError> {
    let collection = match &query.source {
        AxQuerySourcePlan::Stream { collection } => collection,
        AxQuerySourcePlan::ContentCollection { collection } => collection,
        AxQuerySourcePlan::RawSql { .. } => return Err(AxSqlCompileError::RawSqlRuntimeOnly),
    };
    validate_ident(collection)?;

    let mut qualifiers = std::collections::BTreeSet::from([collection.as_str()]);
    for join in &query.joins {
        validate_ident(&join.collection)?;
        if !qualifiers.insert(join.collection.as_str()) {
            return Err(AxSqlCompileError::AmbiguousJoinCollection {
                collection: join.collection.clone(),
            });
        }
        if join.columns.is_empty() {
            return Err(AxSqlCompileError::EmptyJoinColumns {
                collection: join.collection.clone(),
            });
        }
        let mut source_columns = std::collections::BTreeSet::new();
        let mut target_columns = std::collections::BTreeSet::new();
        for column in &join.columns {
            validate_ident(&column.source)?;
            validate_ident(&column.target)?;
            if !source_columns.insert(column.source.as_str())
                || !target_columns.insert(column.target.as_str())
            {
                return Err(AxSqlCompileError::DuplicateJoinColumns {
                    collection: join.collection.clone(),
                });
            }
        }
    }

    let mut sql = if query.joins.is_empty() {
        format!("select * from {}", dialect.quote_ident(collection))
    } else {
        format!(
            "select {}.* from {}",
            dialect.quote_ident(collection),
            dialect.quote_ident(collection)
        )
    };
    for join in &query.joins {
        let clauses = join
            .columns
            .iter()
            .map(|column| {
                format!(
                    "{}.{} = {}.{}",
                    dialect.quote_ident(collection),
                    dialect.quote_ident(&column.source),
                    dialect.quote_ident(&join.collection),
                    dialect.quote_ident(&column.target)
                )
            })
            .collect::<Vec<_>>()
            .join(" and ");
        sql.push_str(&format!(
            " inner join {} on {clauses}",
            dialect.quote_ident(&join.collection)
        ));
    }
    let mut params = Vec::new();

    if !query.filters.is_empty() {
        let mut clauses = Vec::with_capacity(query.filters.len());

        for filter in &query.filters {
            let field = compile_query_field(
                &filter.field,
                collection,
                &qualifiers,
                !query.joins.is_empty(),
                dialect,
            )?;
            clauses.push(compile_filter_clause_with_field(
                filter,
                field,
                dialect,
                &mut params,
                AxSqlFilterContext::Query,
            )?);
        }

        sql.push_str(" where ");
        sql.push_str(&clauses.join(" and "));
    }

    if !query.orders.is_empty() {
        let mut clauses = Vec::with_capacity(query.orders.len());

        for order in &query.orders {
            clauses.push(format!(
                "{} {}",
                compile_query_field(
                    &order.field,
                    collection,
                    &qualifiers,
                    !query.joins.is_empty(),
                    dialect,
                )?,
                order_direction_name(order.direction)
            ));
        }

        sql.push_str(" order by ");
        sql.push_str(&clauses.join(", "));
    }

    if let Some(limit) = query.limit {
        sql.push_str(&format!(" limit {limit}"));
    }

    if let Some(offset) = query.offset {
        sql.push_str(&format!(" offset {offset}"));
    }

    Ok(AxSqlQuery { sql, params })
}

pub fn compile_insert_plan_to_sql(
    collection: &str,
    fields: &[AxAssignmentPlan],
    dialect: AxSqlDialect,
) -> Result<AxSqlMutation, AxSqlCompileError> {
    validate_ident(collection)?;
    if fields.is_empty() {
        return Err(AxSqlCompileError::EmptyMutationFields);
    }

    let mut columns = Vec::with_capacity(fields.len());
    let mut placeholders = Vec::with_capacity(fields.len());
    let mut params = Vec::with_capacity(fields.len());

    for field in fields {
        validate_ident(&field.name)?;
        columns.push(dialect.quote_ident(&field.name));
        placeholders.push(dialect.placeholder(params.len() + 1));
        params.push(AxSqlParam {
            index: params.len() + 1,
            value: field.value.clone(),
        });
    }

    Ok(AxSqlMutation {
        sql: format!(
            "insert into {} ({}) values ({})",
            dialect.quote_ident(collection),
            columns.join(", "),
            placeholders.join(", ")
        ),
        params,
    })
}

pub fn compile_update_plan_to_sql(
    collection: &str,
    fields: &[AxAssignmentPlan],
    filters: &[AxQueryFilterPlan],
    dialect: AxSqlDialect,
) -> Result<AxSqlMutation, AxSqlCompileError> {
    validate_ident(collection)?;
    if fields.is_empty() {
        return Err(AxSqlCompileError::EmptyMutationFields);
    }

    let mut assignments = Vec::with_capacity(fields.len());
    let mut params = Vec::with_capacity(fields.len());

    for field in fields {
        validate_ident(&field.name)?;
        assignments.push(format!(
            "{} = {}",
            dialect.quote_ident(&field.name),
            dialect.placeholder(params.len() + 1)
        ));
        params.push(AxSqlParam {
            index: params.len() + 1,
            value: field.value.clone(),
        });
    }

    let where_clause = if filters.is_empty() {
        String::new()
    } else {
        let mut clauses = Vec::with_capacity(filters.len());

        for filter in filters {
            clauses.push(compile_filter_clause(
                filter,
                dialect,
                &mut params,
                AxSqlFilterContext::Mutation,
            )?);
        }

        format!(" where {}", clauses.join(" and "))
    };

    Ok(AxSqlMutation {
        sql: format!(
            "update {} set {}{}",
            dialect.quote_ident(collection),
            assignments.join(", "),
            where_clause
        ),
        params,
    })
}

pub fn compile_delete_plan_to_sql(
    collection: &str,
    filters: &[AxQueryFilterPlan],
    dialect: AxSqlDialect,
) -> Result<AxSqlMutation, AxSqlCompileError> {
    validate_ident(collection)?;
    if filters.is_empty() {
        return Err(AxSqlCompileError::EmptyDeleteFilters);
    }

    let mut clauses = Vec::with_capacity(filters.len());
    let mut params = Vec::with_capacity(filters.len());

    for filter in filters {
        clauses.push(compile_filter_clause(
            filter,
            dialect,
            &mut params,
            AxSqlFilterContext::Mutation,
        )?);
    }

    Ok(AxSqlMutation {
        sql: format!(
            "delete from {} where {}",
            dialect.quote_ident(collection),
            clauses.join(" and ")
        ),
        params,
    })
}

fn compile_filter_clause(
    filter: &AxQueryFilterPlan,
    dialect: AxSqlDialect,
    params: &mut Vec<AxSqlParam>,
    context: AxSqlFilterContext,
) -> Result<String, AxSqlCompileError> {
    validate_ident(&filter.field)?;
    let field = dialect.quote_ident(&filter.field);

    compile_filter_clause_with_field(filter, field, dialect, params, context)
}

fn compile_filter_clause_with_field(
    filter: &AxQueryFilterPlan,
    field: String,
    dialect: AxSqlDialect,
    params: &mut Vec<AxSqlParam>,
    context: AxSqlFilterContext,
) -> Result<String, AxSqlCompileError> {
    match filter.op {
        AxQueryFilterOpPlan::Eq | AxQueryFilterOpPlan::Ne => {
            let placeholder = push_sql_param(params, filter.value.clone(), dialect);
            let op = match filter.op {
                AxQueryFilterOpPlan::Eq => "=",
                AxQueryFilterOpPlan::Ne => "!=",
                _ => unreachable!("matched equality filter above"),
            };
            Ok(format!("{field} {op} {placeholder}"))
        }
        AxQueryFilterOpPlan::In | AxQueryFilterOpPlan::NotIn => {
            let Some(values) = filter_list_values(&filter.value) else {
                return Err(AxSqlCompileError::NonLiteralListFilter);
            };
            if values.is_empty() {
                return Ok(match filter.op {
                    AxQueryFilterOpPlan::In => "1 = 0".to_string(),
                    AxQueryFilterOpPlan::NotIn if context == AxSqlFilterContext::Query => {
                        "1 = 1".to_string()
                    }
                    AxQueryFilterOpPlan::NotIn => "1 = 0".to_string(),
                    _ => unreachable!("matched list filter above"),
                });
            }
            let placeholders = values
                .into_iter()
                .map(|value| push_sql_param(params, value, dialect))
                .collect::<Vec<_>>()
                .join(", ");
            let op = match filter.op {
                AxQueryFilterOpPlan::In => "in",
                AxQueryFilterOpPlan::NotIn => "not in",
                _ => unreachable!("matched list filter above"),
            };
            Ok(format!("{field} {op} ({placeholders})"))
        }
        AxQueryFilterOpPlan::IsNull => Ok(format!("{field} is null")),
        AxQueryFilterOpPlan::IsNotNull => Ok(format!("{field} is not null")),
    }
}

fn compile_query_field(
    field: &str,
    source: &str,
    qualifiers: &std::collections::BTreeSet<&str>,
    qualify_source: bool,
    dialect: AxSqlDialect,
) -> Result<String, AxSqlCompileError> {
    if let Some((qualifier, column)) = field.split_once('.') {
        validate_ident(qualifier)?;
        validate_ident(column)?;
        if !qualifiers.contains(qualifier) {
            return Err(AxSqlCompileError::UnknownQueryQualifier {
                field: field.to_string(),
                qualifier: qualifier.to_string(),
            });
        }
        return Ok(format!(
            "{}.{}",
            dialect.quote_ident(qualifier),
            dialect.quote_ident(column)
        ));
    }

    validate_ident(field)?;
    if qualify_source {
        Ok(format!(
            "{}.{}",
            dialect.quote_ident(source),
            dialect.quote_ident(field)
        ))
    } else {
        Ok(dialect.quote_ident(field))
    }
}

fn push_sql_param(
    params: &mut Vec<AxSqlParam>,
    value: AxRustExpr,
    dialect: AxSqlDialect,
) -> String {
    let placeholder = dialect.placeholder(params.len() + 1);
    params.push(AxSqlParam {
        index: params.len() + 1,
        value,
    });
    placeholder
}

fn filter_list_values(value: &AxRustExpr) -> Option<Vec<AxRustExpr>> {
    let code = value.code.trim();
    let inner = code
        .strip_prefix("vec![")
        .and_then(|value| value.strip_suffix(']'))
        .or_else(|| {
            code.strip_prefix('[')
                .and_then(|value| value.strip_suffix(']'))
        });
    let inner = inner?;
    if inner.trim().is_empty() {
        return Some(Vec::new());
    }

    Some(
        split_top_level(inner, ',')
            .into_iter()
            .map(|value| AxRustExpr::new(value.trim()))
            .collect(),
    )
}

fn split_top_level(input: &str, delimiter: char) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0usize;
    let mut depth = 0usize;
    let mut in_string: Option<char> = None;
    let mut escaped = false;

    for (index, ch) in input.char_indices() {
        match in_string {
            Some(quote) => {
                if escaped {
                    escaped = false;
                    continue;
                }
                if ch == '\\' {
                    escaped = true;
                    continue;
                }
                if ch == quote {
                    in_string = None;
                }
            }
            None => match ch {
                '"' | '\'' => {
                    in_string = Some(ch);
                    escaped = false;
                }
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth = depth.saturating_sub(1),
                _ if ch == delimiter && depth == 0 => {
                    result.push(input[start..index].trim());
                    start = index + ch.len_utf8();
                }
                _ => {}
            },
        }
    }

    result.push(input[start..].trim());
    result
}

fn validate_ident(ident: &str) -> Result<(), AxSqlCompileError> {
    let trimmed = ident.trim();
    if trimmed.is_empty() {
        return Err(AxSqlCompileError::EmptyCollection);
    }

    if trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        Ok(())
    } else {
        Err(AxSqlCompileError::InvalidIdentifier {
            ident: ident.to_string(),
        })
    }
}

fn order_direction_name(direction: AxQueryOrderDirectionPlan) -> &'static str {
    match direction {
        AxQueryOrderDirectionPlan::Asc => "asc",
        AxQueryOrderDirectionPlan::Desc => "desc",
    }
}

pub mod prelude {
    pub use super::compile_delete_plan_to_sql;
    pub use super::compile_insert_plan_to_sql;
    pub use super::compile_query_plan_to_sql;
    pub use super::compile_update_plan_to_sql;
    pub use super::AxSqlCompileError;
    pub use super::AxSqlDialect;
    pub use super::AxSqlMutation;
    pub use super::AxSqlParam;
    pub use super::AxSqlQuery;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_postgres_query_plan_into_sql() {
        let query = AxQueryPlan {
            source: AxQuerySourcePlan::Stream {
                collection: "posts".to_string(),
            },
            joins: Vec::new(),
            filters: vec![AxQueryFilterPlan {
                field: "status".to_string(),
                op: AxQueryFilterOpPlan::Eq,
                value: AxRustExpr::new(r#""published".to_string()"#),
            }],
            orders: vec![AxQueryOrderPlan {
                field: "created_at".to_string(),
                direction: AxQueryOrderDirectionPlan::Desc,
            }],
            limit: Some(20),
            offset: Some(40),
            mode: AxQueryModePlan::Many,
        };

        let sql = compile_query_plan_to_sql(&query, AxSqlDialect::Postgres)
            .expect("query should compile");

        assert_eq!(
            sql.sql,
            r#"select * from "posts" where "status" = $1 order by "created_at" desc limit 20 offset 40"#
        );
        assert_eq!(
            sql.params,
            vec![AxSqlParam {
                index: 1,
                value: AxRustExpr::new(r#""published".to_string()"#),
            }]
        );
    }

    #[test]
    fn compiles_typed_join_and_qualified_fields_into_sql() {
        let query = AxQueryPlan {
            source: AxQuerySourcePlan::Stream {
                collection: "posts".to_string(),
            },
            joins: vec![AxQueryJoinPlan {
                collection: "authors".to_string(),
                columns: vec![
                    AxQueryJoinColumnPlan {
                        source: "tenant_id".to_string(),
                        target: "tenant_id".to_string(),
                    },
                    AxQueryJoinColumnPlan {
                        source: "author_id".to_string(),
                        target: "id".to_string(),
                    },
                ],
            }],
            filters: vec![
                AxQueryFilterPlan {
                    field: "status".to_string(),
                    op: AxQueryFilterOpPlan::Eq,
                    value: AxRustExpr::new(r#""published".to_string()"#),
                },
                AxQueryFilterPlan {
                    field: "authors.active".to_string(),
                    op: AxQueryFilterOpPlan::Eq,
                    value: AxRustExpr::new("true"),
                },
            ],
            orders: vec![AxQueryOrderPlan {
                field: "authors.name".to_string(),
                direction: AxQueryOrderDirectionPlan::Asc,
            }],
            limit: None,
            offset: None,
            mode: AxQueryModePlan::Many,
        };

        let sql = compile_query_plan_to_sql(&query, AxSqlDialect::Postgres)
            .expect("join query should compile");

        assert_eq!(
            sql.sql,
            r#"select "posts".* from "posts" inner join "authors" on "posts"."tenant_id" = "authors"."tenant_id" and "posts"."author_id" = "authors"."id" where "posts"."status" = $1 and "authors"."active" = $2 order by "authors"."name" asc"#
        );
        assert_eq!(sql.params.len(), 2);
    }

    #[test]
    fn rejects_programmatic_join_with_duplicate_columns() {
        let query = AxQueryPlan {
            source: AxQuerySourcePlan::Stream {
                collection: "posts".to_string(),
            },
            joins: vec![AxQueryJoinPlan {
                collection: "authors".to_string(),
                columns: vec![
                    AxQueryJoinColumnPlan {
                        source: "author_id".to_string(),
                        target: "id".to_string(),
                    },
                    AxQueryJoinColumnPlan {
                        source: "author_id".to_string(),
                        target: "tenant_id".to_string(),
                    },
                ],
            }],
            filters: Vec::new(),
            orders: Vec::new(),
            limit: None,
            offset: None,
            mode: AxQueryModePlan::Many,
        };

        assert_eq!(
            compile_query_plan_to_sql(&query, AxSqlDialect::Postgres),
            Err(AxSqlCompileError::DuplicateJoinColumns {
                collection: "authors".to_string(),
            })
        );
    }

    #[test]
    fn compiles_mysql_query_plan_into_sql() {
        let query = AxQueryPlan {
            source: AxQuerySourcePlan::Stream {
                collection: "posts".to_string(),
            },
            joins: Vec::new(),
            filters: vec![
                AxQueryFilterPlan {
                    field: "status".to_string(),
                    op: AxQueryFilterOpPlan::Eq,
                    value: AxRustExpr::new(r#""published".to_string()"#),
                },
                AxQueryFilterPlan {
                    field: "featured".to_string(),
                    op: AxQueryFilterOpPlan::Eq,
                    value: AxRustExpr::new("true"),
                },
            ],
            orders: vec![AxQueryOrderPlan {
                field: "created_at".to_string(),
                direction: AxQueryOrderDirectionPlan::Desc,
            }],
            limit: Some(12),
            offset: None,
            mode: AxQueryModePlan::Many,
        };

        let sql =
            compile_query_plan_to_sql(&query, AxSqlDialect::MySql).expect("query should compile");

        assert_eq!(
            sql.sql,
            "select * from `posts` where `status` = ? and `featured` = ? order by `created_at` desc limit 12"
        );
        assert_eq!(sql.params.len(), 2);
        assert_eq!(sql.params[0].index, 1);
        assert_eq!(sql.params[1].index, 2);
    }

    #[test]
    fn compiles_query_filter_ops_into_sql() {
        let query = AxQueryPlan {
            source: AxQuerySourcePlan::Stream {
                collection: "posts".to_string(),
            },
            joins: Vec::new(),
            filters: vec![
                AxQueryFilterPlan {
                    field: "archived".to_string(),
                    op: AxQueryFilterOpPlan::Ne,
                    value: AxRustExpr::new("true"),
                },
                AxQueryFilterPlan {
                    field: "status".to_string(),
                    op: AxQueryFilterOpPlan::In,
                    value: AxRustExpr::new(
                        r#"vec!["published".to_string(), "featured".to_string()]"#,
                    ),
                },
                AxQueryFilterPlan {
                    field: "tag".to_string(),
                    op: AxQueryFilterOpPlan::NotIn,
                    value: AxRustExpr::new(r#"["blocked", "hidden"]"#),
                },
                AxQueryFilterPlan {
                    field: "deleted_at".to_string(),
                    op: AxQueryFilterOpPlan::IsNull,
                    value: AxRustExpr::new("true"),
                },
                AxQueryFilterPlan {
                    field: "published_at".to_string(),
                    op: AxQueryFilterOpPlan::IsNotNull,
                    value: AxRustExpr::new("true"),
                },
            ],
            orders: Vec::new(),
            limit: None,
            offset: None,
            mode: AxQueryModePlan::Many,
        };

        let sql = compile_query_plan_to_sql(&query, AxSqlDialect::Postgres)
            .expect("query should compile");

        assert_eq!(
            sql.sql,
            r#"select * from "posts" where "archived" != $1 and "status" in ($2, $3) and "tag" not in ($4, $5) and "deleted_at" is null and "published_at" is not null"#
        );
        assert_eq!(
            sql.params,
            vec![
                AxSqlParam {
                    index: 1,
                    value: AxRustExpr::new("true"),
                },
                AxSqlParam {
                    index: 2,
                    value: AxRustExpr::new(r#""published".to_string()"#),
                },
                AxSqlParam {
                    index: 3,
                    value: AxRustExpr::new(r#""featured".to_string()"#),
                },
                AxSqlParam {
                    index: 4,
                    value: AxRustExpr::new(r#""blocked""#),
                },
                AxSqlParam {
                    index: 5,
                    value: AxRustExpr::new(r#""hidden""#),
                },
            ]
        );
    }

    #[test]
    fn rejects_non_literal_list_filters() {
        let query = AxQueryPlan {
            source: AxQuerySourcePlan::Stream {
                collection: "posts".to_string(),
            },
            joins: Vec::new(),
            filters: vec![AxQueryFilterPlan {
                field: "status".to_string(),
                op: AxQueryFilterOpPlan::In,
                value: AxRustExpr::new("allowed_statuses"),
            }],
            orders: Vec::new(),
            limit: None,
            offset: None,
            mode: AxQueryModePlan::Many,
        };

        let error = compile_query_plan_to_sql(&query, AxSqlDialect::Postgres)
            .expect_err("dynamic list filters should be rejected until runtime binding exists");

        assert_eq!(error, AxSqlCompileError::NonLiteralListFilter);
    }

    #[test]
    fn preserves_escaped_quotes_when_splitting_list_filters() {
        let query = AxQueryPlan {
            source: AxQuerySourcePlan::Stream {
                collection: "posts".to_string(),
            },
            joins: Vec::new(),
            filters: vec![AxQueryFilterPlan {
                field: "slug".to_string(),
                op: AxQueryFilterOpPlan::In,
                value: AxRustExpr::new(r#"["a\"b", "c"]"#),
            }],
            orders: Vec::new(),
            limit: None,
            offset: None,
            mode: AxQueryModePlan::Many,
        };

        let sql = compile_query_plan_to_sql(&query, AxSqlDialect::Postgres)
            .expect("escaped string list should compile");

        assert_eq!(sql.sql, r#"select * from "posts" where "slug" in ($1, $2)"#);
        assert_eq!(
            sql.params,
            vec![
                AxSqlParam {
                    index: 1,
                    value: AxRustExpr::new(r#""a\"b""#),
                },
                AxSqlParam {
                    index: 2,
                    value: AxRustExpr::new(r#""c""#),
                },
            ]
        );
    }

    #[test]
    fn compiles_empty_not_in_as_noop_for_queries_and_noop_mutation_guard() {
        let query = AxQueryPlan {
            source: AxQuerySourcePlan::Stream {
                collection: "posts".to_string(),
            },
            joins: Vec::new(),
            filters: vec![AxQueryFilterPlan {
                field: "status".to_string(),
                op: AxQueryFilterOpPlan::NotIn,
                value: AxRustExpr::new("[]"),
            }],
            orders: Vec::new(),
            limit: None,
            offset: None,
            mode: AxQueryModePlan::Many,
        };

        let sql = compile_query_plan_to_sql(&query, AxSqlDialect::Postgres)
            .expect("empty NOT IN query should compile as a true clause");
        assert_eq!(sql.sql, r#"select * from "posts" where 1 = 1"#);

        let delete = compile_delete_plan_to_sql(
            "posts",
            &[AxQueryFilterPlan {
                field: "status".to_string(),
                op: AxQueryFilterOpPlan::NotIn,
                value: AxRustExpr::new("[]"),
            }],
            AxSqlDialect::Postgres,
        )
        .expect("empty NOT IN delete should compile as a no-op mutation");
        assert_eq!(delete.sql, r#"delete from "posts" where 1 = 0"#);
    }

    #[test]
    fn compiles_sqlite_query_plan_into_sql() {
        let query = AxQueryPlan {
            source: AxQuerySourcePlan::Stream {
                collection: "posts".to_string(),
            },
            joins: Vec::new(),
            filters: Vec::new(),
            orders: vec![AxQueryOrderPlan {
                field: "created_at".to_string(),
                direction: AxQueryOrderDirectionPlan::Asc,
            }],
            limit: Some(5),
            offset: Some(10),
            mode: AxQueryModePlan::Many,
        };

        let sql =
            compile_query_plan_to_sql(&query, AxSqlDialect::Sqlite).expect("query should compile");

        assert_eq!(
            sql.sql,
            r#"select * from "posts" order by "created_at" asc limit 5 offset 10"#
        );
        assert!(sql.params.is_empty());
    }

    #[test]
    fn rejects_invalid_identifiers() {
        let query = AxQueryPlan {
            source: AxQuerySourcePlan::Stream {
                collection: "blog-posts".to_string(),
            },
            joins: Vec::new(),
            filters: Vec::new(),
            orders: Vec::new(),
            limit: None,
            offset: None,
            mode: AxQueryModePlan::Many,
        };

        let error = compile_query_plan_to_sql(&query, AxSqlDialect::Postgres)
            .expect_err("invalid identifier should fail");

        assert_eq!(
            error,
            AxSqlCompileError::InvalidIdentifier {
                ident: "blog-posts".to_string(),
            }
        );
    }

    #[test]
    fn compiles_postgres_insert_plan_into_sql() {
        let mutation = compile_insert_plan_to_sql(
            "posts",
            &[
                AxAssignmentPlan {
                    name: "title".to_string(),
                    value: AxRustExpr::new("input.title"),
                },
                AxAssignmentPlan {
                    name: "featured".to_string(),
                    value: AxRustExpr::new("input.featured"),
                },
            ],
            AxSqlDialect::Postgres,
        )
        .expect("insert should compile");

        assert_eq!(
            mutation.sql,
            r#"insert into "posts" ("title", "featured") values ($1, $2)"#
        );
        assert_eq!(mutation.params.len(), 2);
        assert_eq!(mutation.params[0].index, 1);
        assert_eq!(mutation.params[1].index, 2);
    }

    #[test]
    fn compiles_mysql_update_plan_into_sql() {
        let mutation = compile_update_plan_to_sql(
            "posts",
            &[
                AxAssignmentPlan {
                    name: "title".to_string(),
                    value: AxRustExpr::new("input.title"),
                },
                AxAssignmentPlan {
                    name: "featured".to_string(),
                    value: AxRustExpr::new("input.featured"),
                },
            ],
            &[],
            AxSqlDialect::MySql,
        )
        .expect("update should compile");

        assert_eq!(
            mutation.sql,
            "update `posts` set `title` = ?, `featured` = ?"
        );
        assert_eq!(mutation.params.len(), 2);
    }

    #[test]
    fn compiles_postgres_update_plan_with_where_clause() {
        let mutation = compile_update_plan_to_sql(
            "posts",
            &[AxAssignmentPlan {
                name: "title".to_string(),
                value: AxRustExpr::new("input.title"),
            }],
            &[AxQueryFilterPlan {
                field: "id".to_string(),
                op: AxQueryFilterOpPlan::Eq,
                value: AxRustExpr::new("input.id"),
            }],
            AxSqlDialect::Postgres,
        )
        .expect("update should compile");

        assert_eq!(
            mutation.sql,
            r#"update "posts" set "title" = $1 where "id" = $2"#
        );
        assert_eq!(mutation.params.len(), 2);
    }

    #[test]
    fn rejects_empty_mutation_fields() {
        let error = compile_insert_plan_to_sql("posts", &[], AxSqlDialect::Sqlite)
            .expect_err("empty insert should fail");

        assert_eq!(error, AxSqlCompileError::EmptyMutationFields);
    }

    #[test]
    fn compiles_delete_plan_with_where_clause() {
        let mutation = compile_delete_plan_to_sql(
            "posts",
            &[AxQueryFilterPlan {
                field: "id".to_string(),
                op: AxQueryFilterOpPlan::Eq,
                value: AxRustExpr::new("input.id"),
            }],
            AxSqlDialect::Postgres,
        )
        .expect("delete should compile");

        assert_eq!(mutation.sql, r#"delete from "posts" where "id" = $1"#);
        assert_eq!(mutation.params.len(), 1);
    }

    #[test]
    fn rejects_delete_without_filters() {
        let error = compile_delete_plan_to_sql("posts", &[], AxSqlDialect::Postgres)
            .expect_err("delete without filters should fail");

        assert_eq!(error, AxSqlCompileError::EmptyDeleteFilters);
    }
}
