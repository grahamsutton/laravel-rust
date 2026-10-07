//! Forwarding the query builder's clause methods to Eloquent builders and
//! relations, so they read exactly like the base query builder.

/// Generate by-value clause methods that forward to `self.$field`.
macro_rules! forward_methods {
    ($field:ident; $( $(#[$meta:meta])* $name:ident ( $($arg:ident : $ty:ty),* ) ; )* ) => {
        $(
            $(#[$meta])*
            pub fn $name(mut self, $($arg: $ty),*) -> Self {
                self.$field = self.$field.$name($($arg),*);
                self
            }
        )*
    };
}

/// The base query builder's clause methods (selects, joins, wheres,
/// groups, havings, orders, limits and locks).
macro_rules! forward_clauses {
    ($field:ident) => {
        forward_methods! { $field;
            /// Set the columns to be selected.
            select(columns: impl $crate::IntoColumns);
            /// Add a raw select expression.
            select_raw(expression: &str, bindings: impl $crate::IntoBindings);
            /// Add a sub-select expression.
            select_sub(query: impl $crate::query::IntoSubQuery, alias: &str);
            /// Add columns to the select.
            add_select(columns: impl $crate::IntoColumns);
            /// Only return distinct results.
            distinct();
            /// Add an inner join.
            join(table: impl Into<$crate::Ident>, first: impl Into<$crate::Ident>, operator: &str, second: impl Into<$crate::Ident>);
            /// Add an inner join with a closure building its conditions.
            join_with(table: impl Into<$crate::Ident>, callback: impl FnOnce($crate::JoinClause) -> $crate::JoinClause);
            /// Add a left join.
            left_join(table: impl Into<$crate::Ident>, first: impl Into<$crate::Ident>, operator: &str, second: impl Into<$crate::Ident>);
            /// Add a left join with a closure building its conditions.
            left_join_with(table: impl Into<$crate::Ident>, callback: impl FnOnce($crate::JoinClause) -> $crate::JoinClause);
            /// Add a right join.
            right_join(table: impl Into<$crate::Ident>, first: impl Into<$crate::Ident>, operator: &str, second: impl Into<$crate::Ident>);
            /// Add a cross join.
            cross_join(table: impl Into<$crate::Ident>);
            /// Join a sub-query.
            join_sub(query: impl $crate::query::IntoSubQuery, alias: &str, first: impl Into<$crate::Ident>, operator: &str, second: impl Into<$crate::Ident>);
            /// Left join a sub-query.
            left_join_sub(query: impl $crate::query::IntoSubQuery, alias: &str, first: impl Into<$crate::Ident>, operator: &str, second: impl Into<$crate::Ident>);
            /// Add a basic `=` where clause.
            where_(column: impl Into<$crate::Ident>, value: impl Into<$crate::Operand>);
            /// Add a where clause with an operator.
            where_op(column: impl Into<$crate::Ident>, operator: &str, value: impl Into<$crate::Operand>);
            /// Add an "or where" clause.
            or_where(column: impl Into<$crate::Ident>, value: impl Into<$crate::Operand>);
            /// Add an "or where" clause with an operator.
            or_where_op(column: impl Into<$crate::Ident>, operator: &str, value: impl Into<$crate::Operand>);
            /// Add a where clause per column / value pair.
            where_map(values: impl $crate::IntoRecord);
            /// Add an "or" where clause per column / value pair.
            or_where_map(values: impl $crate::IntoRecord);
            /// Add a nested (parenthesised) where group.
            where_group(callback: impl FnOnce($crate::query::Builder) -> $crate::query::Builder);
            /// Add a nested "or" where group.
            or_where_group(callback: impl FnOnce($crate::query::Builder) -> $crate::query::Builder);
            /// Add a negated nested where group.
            where_not(callback: impl FnOnce($crate::query::Builder) -> $crate::query::Builder);
            /// Add a negated nested "or" where group.
            or_where_not(callback: impl FnOnce($crate::query::Builder) -> $crate::query::Builder);
            /// Compare two columns.
            where_column(first: impl Into<$crate::Ident>, second: impl Into<$crate::Ident>);
            /// Compare two columns with an operator.
            where_column_op(first: impl Into<$crate::Ident>, operator: &str, second: impl Into<$crate::Ident>);
            /// Compare two columns with "or".
            or_where_column(first: impl Into<$crate::Ident>, second: impl Into<$crate::Ident>);
            /// Add a raw where clause.
            where_raw(sql: &str, bindings: impl $crate::IntoBindings);
            /// Add a raw "or where" clause.
            or_where_raw(sql: &str, bindings: impl $crate::IntoBindings);
            /// Add a "where in" clause.
            where_in(column: impl Into<$crate::Ident>, values: impl $crate::query::WhereInValues);
            /// Add an "or where in" clause.
            or_where_in(column: impl Into<$crate::Ident>, values: impl $crate::query::WhereInValues);
            /// Add a "where not in" clause.
            where_not_in(column: impl Into<$crate::Ident>, values: impl $crate::query::WhereInValues);
            /// Add an "or where not in" clause.
            or_where_not_in(column: impl Into<$crate::Ident>, values: impl $crate::query::WhereInValues);
            /// Add a "where null" clause.
            where_null(columns: impl $crate::IntoColumns);
            /// Add an "or where null" clause.
            or_where_null(columns: impl $crate::IntoColumns);
            /// Add a "where not null" clause.
            where_not_null(columns: impl $crate::IntoColumns);
            /// Add an "or where not null" clause.
            or_where_not_null(columns: impl $crate::IntoColumns);
            /// Add a "where between" clause.
            where_between(column: impl Into<$crate::Ident>, values: impl $crate::query::BetweenValues<$crate::Operand>);
            /// Add an "or where between" clause.
            or_where_between(column: impl Into<$crate::Ident>, values: impl $crate::query::BetweenValues<$crate::Operand>);
            /// Add a "where not between" clause.
            where_not_between(column: impl Into<$crate::Ident>, values: impl $crate::query::BetweenValues<$crate::Operand>);
            /// Add an "or where not between" clause.
            or_where_not_between(column: impl Into<$crate::Ident>, values: impl $crate::query::BetweenValues<$crate::Operand>);
            /// Add a "where between columns" clause.
            where_between_columns(column: impl Into<$crate::Ident>, values: impl $crate::query::BetweenValues<$crate::Ident>);
            /// Compare the date part of a column.
            where_date(column: impl Into<$crate::Ident>, value: impl Into<$crate::Operand>);
            /// Compare the date part of a column with an operator.
            where_date_op(column: impl Into<$crate::Ident>, operator: &str, value: impl Into<$crate::Operand>);
            /// Compare the date part of a column with "or".
            or_where_date(column: impl Into<$crate::Ident>, value: impl Into<$crate::Operand>);
            /// Compare the time part of a column.
            where_time(column: impl Into<$crate::Ident>, value: impl Into<$crate::Operand>);
            /// Compare the time part of a column with an operator.
            where_time_op(column: impl Into<$crate::Ident>, operator: &str, value: impl Into<$crate::Operand>);
            /// Compare the day of a column.
            where_day(column: impl Into<$crate::Ident>, value: impl Into<$crate::Operand>);
            /// Compare the day of a column with an operator.
            where_day_op(column: impl Into<$crate::Ident>, operator: &str, value: impl Into<$crate::Operand>);
            /// Compare the month of a column.
            where_month(column: impl Into<$crate::Ident>, value: impl Into<$crate::Operand>);
            /// Compare the month of a column with an operator.
            where_month_op(column: impl Into<$crate::Ident>, operator: &str, value: impl Into<$crate::Operand>);
            /// Compare the year of a column.
            where_year(column: impl Into<$crate::Ident>, value: impl Into<$crate::Operand>);
            /// Compare the year of a column with an operator.
            where_year_op(column: impl Into<$crate::Ident>, operator: &str, value: impl Into<$crate::Operand>);
            /// Add a "where exists" clause.
            where_exists(query: impl $crate::query::IntoQuery);
            /// Add an "or where exists" clause.
            or_where_exists(query: impl $crate::query::IntoQuery);
            /// Add a "where not exists" clause.
            where_not_exists(query: impl $crate::query::IntoQuery);
            /// Add an "or where not exists" clause.
            or_where_not_exists(query: impl $crate::query::IntoQuery);
            /// Compare a column against a sub-query.
            where_sub(column: impl Into<$crate::Ident>, operator: &str, query: impl $crate::query::IntoQuery);
            /// Add a "where like" clause.
            where_like(column: impl Into<$crate::Ident>, value: impl Into<$crate::Operand>);
            /// Add an "or where like" clause.
            or_where_like(column: impl Into<$crate::Ident>, value: impl Into<$crate::Operand>);
            /// Add a "where not like" clause.
            where_not_like(column: impl Into<$crate::Ident>, value: impl Into<$crate::Operand>);
            /// Add an "or where not like" clause.
            or_where_not_like(column: impl Into<$crate::Ident>, value: impl Into<$crate::Operand>);
            /// Match when any of the columns match.
            where_any(columns: impl $crate::IntoColumns, operator: &str, value: impl Into<$crate::Operand>);
            /// Match with "or" when any of the columns match.
            or_where_any(columns: impl $crate::IntoColumns, operator: &str, value: impl Into<$crate::Operand>);
            /// Match when all of the columns match.
            where_all(columns: impl $crate::IntoColumns, operator: &str, value: impl Into<$crate::Operand>);
            /// Match with "or" when all of the columns match.
            or_where_all(columns: impl $crate::IntoColumns, operator: &str, value: impl Into<$crate::Operand>);
            /// Match when none of the columns match.
            where_none(columns: impl $crate::IntoColumns, operator: &str, value: impl Into<$crate::Operand>);
            /// Add a "where JSON contains" clause.
            where_json_contains(column: &str, value: impl Into<$crate::Operand>);
            /// Add an "or where JSON contains" clause.
            or_where_json_contains(column: &str, value: impl Into<$crate::Operand>);
            /// Add a "where JSON doesn't contain" clause.
            where_json_doesnt_contain(column: &str, value: impl Into<$crate::Operand>);
            /// Add a "where JSON contains key" clause.
            where_json_contains_key(column: &str);
            /// Add a "where JSON doesn't contain key" clause.
            where_json_doesnt_contain_key(column: &str);
            /// Compare the length of a JSON array.
            where_json_length(column: &str, value: impl Into<$crate::Operand>);
            /// Compare the length of a JSON array with an operator.
            where_json_length_op(column: &str, operator: &str, value: impl Into<$crate::Operand>);
            /// Add a full text search clause.
            where_fulltext(columns: impl $crate::IntoColumns, value: impl Into<$crate::Operand>);
            /// Group the results.
            group_by(groups: impl $crate::IntoColumns);
            /// Add a raw group by clause.
            group_by_raw(sql: &str, bindings: impl $crate::IntoBindings);
            /// Add a "having" clause.
            having(column: impl Into<$crate::Ident>, value: impl Into<$crate::Operand>);
            /// Add a "having" clause with an operator.
            having_op(column: impl Into<$crate::Ident>, operator: &str, value: impl Into<$crate::Operand>);
            /// Add an "or having" clause.
            or_having(column: impl Into<$crate::Ident>, value: impl Into<$crate::Operand>);
            /// Add a "having null" clause.
            having_null(columns: impl $crate::IntoColumns);
            /// Add a "having not null" clause.
            having_not_null(columns: impl $crate::IntoColumns);
            /// Add a "having between" clause.
            having_between(column: impl Into<$crate::Ident>, values: impl $crate::query::BetweenValues<$crate::Operand>);
            /// Add a raw "having" clause.
            having_raw(sql: &str, bindings: impl $crate::IntoBindings);
            /// Add a raw "or having" clause.
            or_having_raw(sql: &str, bindings: impl $crate::IntoBindings);
            /// Order the results.
            order_by(column: impl Into<$crate::Ident>, direction: &str);
            /// Order the results descending.
            order_by_desc(column: impl Into<$crate::Ident>);
            /// Order by a raw expression.
            order_by_raw(sql: &str, bindings: impl $crate::IntoBindings);
            /// Order by a sub-query.
            order_by_sub(query: impl $crate::query::IntoSubQuery, direction: &str);
            /// Order by the given column, newest first.
            latest_by(column: impl Into<$crate::Ident>);
            /// Order by the given column, oldest first.
            oldest_by(column: impl Into<$crate::Ident>);
            /// Order the results randomly.
            in_random_order();
            /// Remove all orderings.
            reorder();
            /// Replace all orderings with one.
            reorder_by(column: impl Into<$crate::Ident>, direction: &str);
            /// Skip the given number of results.
            offset(value: i64);
            /// Alias of `offset`.
            skip(value: i64);
            /// Limit the number of results.
            limit(value: i64);
            /// Alias of `limit`.
            take(value: i64);
            /// Constrain the query to a page of results.
            for_page(page: i64, per_page: i64);
            /// Lock the selected rows for update.
            lock_for_update();
            /// Share lock the selected rows.
            shared_lock();
            /// Run reads on the write connection.
            use_write_pdo();
        }
    };
}

/// Eloquent builder methods, forwarded from relations to their query.
macro_rules! forward_eloquent {
    ($field:ident) => {
        forward_clauses!($field);

        forward_methods! { $field;
            /// Order by the related model's "created at" column, newest first.
            latest();
            /// Order by the related model's "created at" column, oldest first.
            oldest();
            /// Eager load relationships on the related models.
            with(relations: impl $crate::eloquent::IntoRelations);
            /// Eager load a relationship with a constraint.
            with_constrained(relation: &str, constraint: impl Fn($crate::query::Builder) -> $crate::query::Builder + Send + Sync + 'static);
            /// Add `{relation}_count` columns.
            with_count(relations: impl $crate::eloquent::IntoRelations);
            /// Only related models that have the relationship.
            has(relation: &str);
            /// Only related models whose relationship matches the constraint.
            where_has(relation: &str, constraint: impl FnOnce($crate::query::Builder) -> $crate::query::Builder);
            /// Only related models without the relationship.
            doesnt_have(relation: &str);
            /// Include soft deleted related models.
            with_trashed();
            /// Only soft deleted related models.
            only_trashed();
            /// Remove a global scope.
            without_global_scope(name: &str);
            /// Remove every global scope.
            without_global_scopes();
            /// Apply a local scope.
            scope(scope: impl FnOnce($crate::eloquent::Builder<R>) -> $crate::eloquent::Builder<R>);
        }
    };
}

/// Retrieval and aggregate methods shared by every relation. The relation
/// provides `get_query()` (the constrained related query) and `fetch()`
/// (run a query and hydrate the related models).
macro_rules! relation_queries {
    () => {
        /// Get the first related model.
        pub async fn first(&self) -> ::illuminate_support::Result<Option<R>> {
            Ok(self
                .fetch(self.get_query().take(1))
                .await?
                .into_iter()
                .next())
        }

        /// Get the first related model or fail with a `ModelNotFoundException`.
        pub async fn first_or_fail(&self) -> ::illuminate_support::Result<R> {
            self.first().await?.ok_or_else(|| {
                $crate::eloquent::ModelNotFoundException::new(R::class_name(), Vec::new()).into()
            })
        }

        /// Find a related model by its primary key.
        pub async fn find(
            &self,
            id: impl Into<::illuminate_support::Value>,
        ) -> ::illuminate_support::Result<Option<R>> {
            Ok(self
                .fetch(self.get_query().where_key(id).take(1))
                .await?
                .into_iter()
                .next())
        }

        /// Find a related model by its primary key or fail with a
        /// `ModelNotFoundException`.
        pub async fn find_or_fail(
            &self,
            id: impl Into<::illuminate_support::Value>,
        ) -> ::illuminate_support::Result<R> {
            let id = id.into();
            self.find(id.clone()).await?.ok_or_else(|| {
                $crate::eloquent::ModelNotFoundException::new(R::class_name(), vec![id]).into()
            })
        }

        /// Find related models by their primary keys.
        pub async fn find_many(
            &self,
            ids: impl $crate::eloquent::IntoIds,
        ) -> ::illuminate_support::Result<::illuminate_support::Collection<R>> {
            let ids = ids.into_ids();
            if ids.is_empty() {
                return Ok(::illuminate_support::Collection::new());
            }
            Ok(self
                .fetch(
                    self.get_query()
                        .where_key(::illuminate_support::Value::Array(ids)),
                )
                .await?
                .into())
        }

        /// Paginate the related models.
        pub async fn paginate(
            &self,
            per_page: impl Into<Option<u64>>,
        ) -> ::illuminate_support::Result<::illuminate_pagination::LengthAwarePaginator<R>> {
            let per_page = per_page.into().unwrap_or_else(R::per_page).max(1);
            let page = ::illuminate_pagination::current_page("page").max(1);
            let query = self.get_query();
            let total = query.to_base().get_count_for_pagination().await?.max(0) as u64;
            let items = if total > 0 {
                self.fetch(query.for_page(page as i64, per_page as i64))
                    .await?
            } else {
                Vec::new()
            };
            let options = ::illuminate_pagination::PaginatorOptions::path(
                ::illuminate_pagination::current_path(),
            );
            Ok(::illuminate_pagination::LengthAwarePaginator::new(
                items, total, per_page, page, options,
            ))
        }

        /// Paginate the related models without counting the total.
        pub async fn simple_paginate(
            &self,
            per_page: impl Into<Option<u64>>,
        ) -> ::illuminate_support::Result<::illuminate_pagination::Paginator<R>> {
            let per_page = per_page.into().unwrap_or_else(R::per_page).max(1);
            let page = ::illuminate_pagination::current_page("page").max(1);
            let query = self
                .get_query()
                .skip(((page - 1) * per_page) as i64)
                .take(per_page as i64 + 1);
            let items = self.fetch(query).await?;
            let options = ::illuminate_pagination::PaginatorOptions::path(
                ::illuminate_pagination::current_path(),
            );
            Ok(::illuminate_pagination::Paginator::new(
                items, per_page, page, options,
            ))
        }

        /// Count the related models.
        pub async fn count(&self) -> ::illuminate_support::Result<i64> {
            self.get_query().count().await
        }

        /// Determine whether any related models exist.
        pub async fn exists(&self) -> ::illuminate_support::Result<bool> {
            self.get_query().exists().await
        }

        /// Determine whether no related models exist.
        pub async fn doesnt_exist(&self) -> ::illuminate_support::Result<bool> {
            self.get_query().doesnt_exist().await
        }

        /// Get a column's values from the related models.
        pub async fn pluck(
            &self,
            column: impl Into<$crate::Ident>,
        ) -> ::illuminate_support::Result<
            ::illuminate_support::Collection<::illuminate_support::Value>,
        > {
            self.get_query().pluck(column).await
        }

        /// Get a column's value from the first related model.
        pub async fn value(
            &self,
            column: impl Into<$crate::Ident>,
        ) -> ::illuminate_support::Result<Option<::illuminate_support::Value>> {
            self.get_query().value(column).await
        }

        /// The sum of a column of the related models.
        pub async fn sum(
            &self,
            column: impl Into<$crate::Ident>,
        ) -> ::illuminate_support::Result<::illuminate_support::Value> {
            self.get_query().sum(column).await
        }

        /// The minimum of a column of the related models.
        pub async fn min(
            &self,
            column: impl Into<$crate::Ident>,
        ) -> ::illuminate_support::Result<::illuminate_support::Value> {
            self.get_query().min(column).await
        }

        /// The maximum of a column of the related models.
        pub async fn max(
            &self,
            column: impl Into<$crate::Ident>,
        ) -> ::illuminate_support::Result<::illuminate_support::Value> {
            self.get_query().max(column).await
        }

        /// The average of a column of the related models.
        pub async fn avg(
            &self,
            column: impl Into<$crate::Ident>,
        ) -> ::illuminate_support::Result<::illuminate_support::Value> {
            self.get_query().avg(column).await
        }

        /// The relationship query's SQL.
        pub fn to_sql(&self) -> String {
            self.get_query().to_sql()
        }
    };
}
