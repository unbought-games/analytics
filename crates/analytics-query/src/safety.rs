use sqlparser::{
    ast::{self, Visit, Visitor},
    dialect::PostgreSqlDialect,
    parser::Parser,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ops::ControlFlow,
};

/// Administrative allowlist. Database grants must independently match it.
pub type Relations = BTreeMap<String, BTreeSet<String>>;

/// Parse and re-render one analytical query; never execute the original text.
/// Deliberately supports SELECT/CTE/join/window/aggregate/set operations, not
/// PostgreSQL's full executable language or arbitrary extension types/functions.
pub fn validate_sql(sql: &str, relations: &Relations) -> Result<String, &'static str> {
    if sql.len() > 64 * 1024 {
        return Err("query_too_large");
    }
    let statements = Parser::parse_sql(&PostgreSqlDialect {}, sql).map_err(|_| "invalid_sql")?;
    if statements.len() != 1 {
        return Err("one_query_required");
    }
    let ast::Statement::Query(query) = &statements[0] else {
        return Err("read_only_query_required");
    };
    if query.order_by.is_none() {
        return Err("explicit_order_required");
    }
    let mut validator = Validator {
        relations,
        ctes: BTreeSet::new(),
        stack: Vec::new(),
        overrides: HashMap::new(),
    };
    if let ControlFlow::Break(error) = statements[0].visit(&mut validator) {
        return Err(error);
    }
    Ok(statements[0].to_string())
}

struct Validator<'a> {
    relations: &'a Relations,
    ctes: BTreeSet<String>,
    stack: Vec<BTreeSet<String>>,
    overrides: HashMap<usize, BTreeSet<String>>,
}
impl Visitor for Validator<'_> {
    type Break = &'static str;
    fn pre_visit_statement(&mut self, statement: &ast::Statement) -> ControlFlow<Self::Break> {
        if !matches!(statement, ast::Statement::Query(_)) {
            return ControlFlow::Break("read_only_query_required");
        }
        ControlFlow::Continue(())
    }
    fn pre_visit_query(&mut self, query: &ast::Query) -> ControlFlow<Self::Break> {
        if !query.locks.is_empty() || query.for_clause.is_some() {
            return ControlFlow::Break("locking_not_allowed");
        }
        self.stack.push(self.ctes.clone());
        if let Some(scope) = self
            .overrides
            .remove(&(query as *const ast::Query as usize))
        {
            self.ctes = scope;
        }
        if let Some(with) = &query.with {
            if with.recursive {
                return ControlFlow::Break("recursive_query_not_allowed");
            }
            for cte in &with.cte_tables {
                self.overrides.insert(
                    cte.query.as_ref() as *const ast::Query as usize,
                    self.ctes.clone(),
                );
                self.ctes.insert(cte.alias.name.to_string());
            }
        }
        fn body_safe(body: &ast::SetExpr) -> bool {
            match body {
                ast::SetExpr::Select(_) | ast::SetExpr::Query(_) | ast::SetExpr::Values(_) => true,
                ast::SetExpr::SetOperation { left, right, .. } => {
                    body_safe(left) && body_safe(right)
                }
                _ => false,
            }
        }
        if !body_safe(&query.body) {
            return ControlFlow::Break("read_only_query_required");
        }
        ControlFlow::Continue(())
    }
    fn post_visit_query(&mut self, _: &ast::Query) -> ControlFlow<Self::Break> {
        self.ctes = self.stack.pop().unwrap_or_default();
        ControlFlow::Continue(())
    }
    fn pre_visit_select(&mut self, select: &ast::Select) -> ControlFlow<Self::Break> {
        if select.into.is_some() {
            return ControlFlow::Break("select_into_not_allowed");
        }
        ControlFlow::Continue(())
    }
    fn pre_visit_relation(&mut self, name: &ast::ObjectName) -> ControlFlow<Self::Break> {
        let name = name.to_string();
        if !self.relations.contains_key(&name) && !self.ctes.contains(&name) {
            return ControlFlow::Break("relation_not_allowed");
        }
        ControlFlow::Continue(())
    }
    fn pre_visit_table_factor(&mut self, table: &ast::TableFactor) -> ControlFlow<Self::Break> {
        match table {
            ast::TableFactor::Table {
                args: None,
                version: None,
                with_hints,
                ..
            } if with_hints.is_empty() => (),
            ast::TableFactor::Derived { .. } | ast::TableFactor::NestedJoin { .. } => (),
            _ => return ControlFlow::Break("table_function_not_allowed"),
        }
        ControlFlow::Continue(())
    }
    fn pre_visit_expr(&mut self, expr: &ast::Expr) -> ControlFlow<Self::Break> {
        use ast::Expr::*;
        match expr {
            Function(f) => {
                let name = f.name.to_string().to_lowercase();
                let name = name.strip_prefix("pg_catalog.").unwrap_or(&name);
                if !matches!(
                    name,
                    "count"
                        | "sum"
                        | "avg"
                        | "min"
                        | "max"
                        | "abs"
                        | "round"
                        | "floor"
                        | "ceil"
                        | "ceiling"
                        | "coalesce"
                        | "nullif"
                        | "greatest"
                        | "least"
                        | "lower"
                        | "upper"
                        | "length"
                        | "substring"
                        | "date_trunc"
                        | "date_part"
                        | "row_number"
                        | "rank"
                        | "dense_rank"
                        | "lag"
                        | "lead"
                        | "first_value"
                        | "last_value"
                        | "percentile_cont"
                        | "percentile_disc"
                        | "bool_and"
                        | "bool_or"
                        | "string_agg"
                        | "array_agg"
                        | "json_agg"
                        | "jsonb_agg"
                ) {
                    return ControlFlow::Break("function_not_allowed");
                }
            }
            Cast { data_type, .. } => {
                if !matches!(
                    data_type.to_string().to_lowercase().as_str(),
                    "text"
                        | "boolean"
                        | "smallint"
                        | "int"
                        | "integer"
                        | "bigint"
                        | "numeric"
                        | "decimal"
                        | "real"
                        | "double precision"
                        | "date"
                        | "timestamp"
                        | "timestamp with time zone"
                        | "timestamptz"
                        | "uuid"
                        | "json"
                        | "jsonb"
                ) {
                    return ControlFlow::Break("cast_not_allowed");
                }
            }
            BinaryOp { op, .. } => {
                if !matches!(
                    op,
                    ast::BinaryOperator::Plus
                        | ast::BinaryOperator::Minus
                        | ast::BinaryOperator::Multiply
                        | ast::BinaryOperator::Divide
                        | ast::BinaryOperator::Modulo
                        | ast::BinaryOperator::StringConcat
                        | ast::BinaryOperator::Gt
                        | ast::BinaryOperator::Lt
                        | ast::BinaryOperator::GtEq
                        | ast::BinaryOperator::LtEq
                        | ast::BinaryOperator::Eq
                        | ast::BinaryOperator::NotEq
                        | ast::BinaryOperator::And
                        | ast::BinaryOperator::Or
                ) {
                    return ControlFlow::Break("operator_not_allowed");
                }
            }
            Identifier(_)
            | CompoundIdentifier(_)
            | Value(_)
            | Nested(_)
            | IsNull(_)
            | IsNotNull(_)
            | IsTrue(_)
            | IsFalse(_)
            | IsNotTrue(_)
            | IsNotFalse(_)
            | IsDistinctFrom(_, _)
            | IsNotDistinctFrom(_, _)
            | InList { .. }
            | InSubquery { .. }
            | Between { .. }
            | Like { .. }
            | ILike { .. }
            | UnaryOp { .. }
            | Case { .. }
            | Exists { .. }
            | Subquery(_)
            | Extract { .. }
            | Interval(_)
            | Tuple(_) => (),
            _ => return ControlFlow::Break("expression_not_allowed"),
        }
        ControlFlow::Continue(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scope() -> Relations {
        BTreeMap::from([(
            "reporting.activity".into(),
            BTreeSet::from(["id".into(), "amount".into()]),
        )])
    }
    #[test]
    fn analytical_queries_and_hostile_inputs() {
        for query in [
            "SELECT id, sum(amount) FROM reporting.activity GROUP BY id ORDER BY id",
            "WITH a AS (SELECT id FROM reporting.activity) SELECT id, row_number() OVER (ORDER BY id) FROM a ORDER BY id",
        ] {
            assert!(validate_sql(query, &scope()).is_ok(), "{query}");
        }
        for query in [
            "WITH pg_roles AS (SELECT * FROM pg_roles) SELECT * FROM pg_roles ORDER BY 1",
            "SELECT (WITH pg_roles AS (SELECT 1) SELECT 1) FROM pg_roles ORDER BY 1",
            "DELETE FROM reporting.activity",
            "SELECT pg_sleep(10) ORDER BY 1",
            "SELECT pg_catalog.pg_read_file('/x') ORDER BY 1",
            "SELECT * FROM pg_catalog.pg_authid ORDER BY 1",
            "WITH d AS (DELETE FROM reporting.activity RETURNING id) SELECT * FROM d ORDER BY 1",
            "SELECT 1; SELECT 2",
            "SELECT id INTO x FROM reporting.activity ORDER BY 1",
            "SELECT id FROM reporting.activity ORDER BY 1 FOR UPDATE",
            "SELECT 1::regprocedure ORDER BY 1",
            "SELECT * FROM generate_series(1,100) ORDER BY 1",
            "SELECT 'x' COLLATE public.custom ORDER BY 1",
            "SELECT 1 OPERATOR(public.+) 2 ORDER BY 1",
        ] {
            assert!(validate_sql(query, &scope()).is_err(), "{query}");
        }
    }
}
