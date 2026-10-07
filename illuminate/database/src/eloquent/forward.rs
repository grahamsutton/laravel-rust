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
            /// Add a nested where statement with the given boolean.
            where_nested(callback: impl FnOnce($crate::query::Builder) -> $crate::query::Builder, boolean: &str);
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
            /// Add a select expression with an alias.
            select_expression(expression: impl Into<$crate::Ident>, alias: &str);
            /// Select the vector distance to the given vector.
            select_vector_distance(column: impl Into<$crate::Ident>, vector: impl $crate::query::IntoEmbedding, alias: Option<&str>);
            /// Only rows within the vector distance.
            where_vector_distance_less_than(column: impl Into<$crate::Ident>, vector: impl $crate::query::IntoEmbedding, max_distance: f64);
            /// Add an "or" vector distance clause.
            or_where_vector_distance_less_than(column: impl Into<$crate::Ident>, vector: impl $crate::query::IntoEmbedding, max_distance: f64);
            /// Only rows similar to the vector, most similar first.
            where_vector_similar_to(column: impl Into<$crate::Ident>, vector: impl $crate::query::IntoEmbedding, min_similarity: f64);
            /// Only rows similar to the vector, optionally ordered by similarity.
            where_vector_similar_to_with(column: impl Into<$crate::Ident>, vector: impl $crate::query::IntoEmbedding, min_similarity: f64, order: bool);
            /// Order by the distance to the vector.
            order_by_vector_distance(column: impl Into<$crate::Ident>, vector: impl $crate::query::IntoEmbedding);
            /// Suggest an index.
            use_index(index: &str);
            /// Force an index.
            force_index(index: &str);
            /// Ignore an index.
            ignore_index(index: &str);
            /// Add a lateral join.
            join_lateral(query: impl $crate::query::IntoSubQuery, alias: &str);
            /// Add a lateral left join.
            left_join_lateral(query: impl $crate::query::IntoSubQuery, alias: &str);
            /// Add a straight join.
            straight_join(table: impl Into<$crate::Ident>, first: impl Into<$crate::Ident>, operator: &str, second: impl Into<$crate::Ident>);
            /// Add a straight join comparing a column with a value.
            straight_join_where(table: impl Into<$crate::Ident>, first: impl Into<$crate::Ident>, operator: &str, second: impl Into<$crate::Operand>);
            /// Add a sub-query straight join.
            straight_join_sub(query: impl $crate::query::IntoSubQuery, alias: &str, first: impl Into<$crate::Ident>, operator: &str, second: impl Into<$crate::Ident>);
            /// Add a right join with a closure building its conditions.
            right_join_with(table: impl Into<$crate::Ident>, callback: impl FnOnce($crate::JoinClause) -> $crate::JoinClause);
            /// Add an inner join comparing a column with a value.
            join_where(table: impl Into<$crate::Ident>, first: impl Into<$crate::Ident>, operator: &str, second: impl Into<$crate::Operand>);
            /// Add a left join comparing a column with a value.
            left_join_where(table: impl Into<$crate::Ident>, first: impl Into<$crate::Ident>, operator: &str, second: impl Into<$crate::Operand>);
            /// Add a "where binary" clause.
            where_binary(column: impl Into<$crate::Ident>, value: impl Into<$crate::Operand>);
            /// Add an "or where binary" clause.
            or_where_binary(column: impl Into<$crate::Ident>, value: impl Into<$crate::Operand>);
            /// Add a "where not binary" clause.
            where_not_binary(column: impl Into<$crate::Ident>, value: impl Into<$crate::Operand>);
            /// Add an "or where not binary" clause.
            or_where_not_binary(column: impl Into<$crate::Ident>, value: impl Into<$crate::Operand>);
            /// Add a null-safe equality clause.
            where_null_safe_equals(column: impl Into<$crate::Ident>, value: impl Into<$crate::Operand>);
            /// Add an "or" null-safe equality clause.
            or_where_null_safe_equals(column: impl Into<$crate::Ident>, value: impl Into<$crate::Operand>);
            /// Compare a row of columns with a row of values.
            where_row_values(columns: impl $crate::IntoColumns, operator: &str, values: impl $crate::IntoBindings);
            /// Add an "or" row values clause.
            or_where_row_values(columns: impl $crate::IntoColumns, operator: &str, values: impl $crate::IntoBindings);
            /// Only rows where the value lies between two columns.
            where_value_between(value: impl Into<$crate::Operand>, columns: impl $crate::query::BetweenValues<$crate::Ident>);
            /// Add an "or" value-between-columns clause.
            or_where_value_between(value: impl Into<$crate::Operand>, columns: impl $crate::query::BetweenValues<$crate::Ident>);
            /// Only rows where the value lies outside two columns.
            where_value_not_between(value: impl Into<$crate::Operand>, columns: impl $crate::query::BetweenValues<$crate::Ident>);
            /// Add an "or" value-not-between-columns clause.
            or_where_value_not_between(value: impl Into<$crate::Operand>, columns: impl $crate::query::BetweenValues<$crate::Ident>);
            /// Add an "or where between columns" clause.
            or_where_between_columns(column: impl Into<$crate::Ident>, values: impl $crate::query::BetweenValues<$crate::Ident>);
            /// Add a "where not between columns" clause.
            where_not_between_columns(column: impl Into<$crate::Ident>, values: impl $crate::query::BetweenValues<$crate::Ident>);
            /// Add an "or where JSON doesn't contain" clause.
            or_where_json_doesnt_contain(column: &str, value: impl Into<$crate::Operand>);
            /// Add an "or where JSON contains key" clause.
            or_where_json_contains_key(column: &str);
            /// Add an "or where JSON doesn't contain key" clause.
            or_where_json_doesnt_contain_key(column: &str);
            /// Add a "where JSON overlaps" clause.
            where_json_overlaps(column: &str, value: impl Into<$crate::Operand>);
            /// Add an "or where JSON overlaps" clause.
            or_where_json_overlaps(column: &str, value: impl Into<$crate::Operand>);
            /// Add a "where JSON doesn't overlap" clause.
            where_json_doesnt_overlap(column: &str, value: impl Into<$crate::Operand>);
            /// Add an "or where JSON doesn't overlap" clause.
            or_where_json_doesnt_overlap(column: &str, value: impl Into<$crate::Operand>);
            /// Compare the length of a JSON array with "or".
            or_where_json_length(column: &str, value: impl Into<$crate::Operand>);
            /// Compare the length of a JSON array with an operator and "or".
            or_where_json_length_op(column: &str, operator: &str, value: impl Into<$crate::Operand>);
            /// Add a full text search clause.
            where_full_text(columns: impl $crate::IntoColumns, value: impl Into<$crate::Operand>);
            /// Add a full text search clause with options.
            where_full_text_with(columns: impl $crate::IntoColumns, value: impl Into<$crate::Operand>, options: $crate::query::FullTextOptions);
            /// Add an "or" full text search clause.
            or_where_full_text(columns: impl $crate::IntoColumns, value: impl Into<$crate::Operand>);
            /// Add an "or" full text search clause with options.
            or_where_full_text_with(columns: impl $crate::IntoColumns, value: impl Into<$crate::Operand>, options: $crate::query::FullTextOptions);
            /// Add an "or where none" clause.
            or_where_none(columns: impl $crate::IntoColumns, operator: &str, value: impl Into<$crate::Operand>);
            /// Add an "or where" clause comparing two columns with an operator.
            or_where_column_op(first: impl Into<$crate::Ident>, operator: &str, second: impl Into<$crate::Ident>);
            /// Add an "or having" clause with an operator.
            or_having_op(column: impl Into<$crate::Ident>, operator: &str, value: impl Into<$crate::Operand>);
            /// Add an "or having null" clause.
            or_having_null(columns: impl $crate::IntoColumns);
            /// Add an "or having not null" clause.
            or_having_not_null(columns: impl $crate::IntoColumns);
            /// Add a "having not between" clause.
            having_not_between(column: impl Into<$crate::Ident>, values: impl $crate::query::BetweenValues<$crate::Operand>);
            /// Add an "or having between" clause.
            or_having_between(column: impl Into<$crate::Ident>, values: impl $crate::query::BetweenValues<$crate::Operand>);
            /// Add an "or having not between" clause.
            or_having_not_between(column: impl Into<$crate::Ident>, values: impl $crate::query::BetweenValues<$crate::Operand>);
            /// Add a nested "having" group.
            having_nested(callback: impl FnOnce($crate::query::Builder) -> $crate::query::Builder);
            /// Add a nested "or having" group.
            or_having_nested(callback: impl FnOnce($crate::query::Builder) -> $crate::query::Builder);
            /// Order by a given sequence of values.
            in_order_of(column: impl Into<$crate::Ident>, values: impl $crate::IntoBindings);
            /// Replace all orderings with one, descending.
            reorder_desc(column: impl Into<$crate::Ident>);
            /// Limit the number of rows per group.
            group_limit(value: i64, column: &str);
            /// Set a query execution timeout in seconds.
            timeout(seconds: impl Into<Option<u64>>);
            /// Register a callback run on the base query before it executes.
            before_query(callback: impl Fn($crate::query::Builder) -> $crate::query::Builder + Send + Sync + 'static);
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
            /// Remove every global scope except the given ones.
            without_global_scopes_except(scopes: impl $crate::eloquent::IntoRelations);
            /// Register a global scope on this query only.
            with_global_scope(name: &str, scope: impl Fn($crate::eloquent::Builder<R>) -> $crate::eloquent::Builder<R> + Send + Sync + 'static);
            /// Register a callback run on the related models.
            after_query(callback: impl Fn(::illuminate_support::Collection<R>) -> ::illuminate_support::Collection<R> + Send + Sync + 'static);
            /// Stop eager loading the given relationships.
            without_eager_load(relations: impl $crate::eloquent::IntoRelations);
            /// Stop eager loading every relationship.
            without_eager_loads();
            /// Eager load relationships matching a constraint, and only keep
            /// the related models that have them.
            with_where_has(relation: &str, constraint: impl Fn($crate::query::Builder) -> $crate::query::Builder + Send + Sync + 'static);
            /// Only related models whose relationship has the column value,
            /// eager loading the matching related models.
            with_where_relation(relation: &str, column: &str, value: impl Into<$crate::Operand> + Clone + Send + Sync + 'static);
            /// Only related models without a related model whose column has the value.
            where_doesnt_have_relation(relation: &str, column: &str, value: impl Into<$crate::Operand>);
            /// Add an "or" clause matching the given primary key(s).
            or_where_key(id: impl Into<::illuminate_support::Value>);
            /// Add an "or" clause excluding the given primary key(s).
            or_where_key_not(id: impl Into<::illuminate_support::Value>);
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

        /// Find a related model by its primary key, or call the callback
        /// when there is none.
        pub async fn find_or<F, Fut, E>(
            &self,
            id: impl Into<::illuminate_support::Value>,
            callback: F,
        ) -> ::illuminate_support::Result<R>
        where
            F: FnOnce() -> Fut,
            Fut: ::std::future::Future<Output = ::std::result::Result<R, E>>,
            E: Into<::illuminate_support::Error>,
        {
            match self.find(id).await? {
                Some(model) => Ok(model),
                None => callback().await.map_err(Into::into),
            }
        }

        /// Find the only related model with the primary key, failing when
        /// there is none or several.
        pub async fn find_sole(
            &self,
            id: impl Into<::illuminate_support::Value>,
        ) -> ::illuminate_support::Result<R> {
            let id = id.into();
            let mut models = self
                .fetch(self.get_query().where_key(id.clone()).take(2))
                .await?;
            match models.len() {
                0 => Err(
                    $crate::eloquent::ModelNotFoundException::new(R::class_name(), vec![id]).into(),
                ),
                1 => Ok(models.remove(0)),
                count => Err($crate::MultipleRecordsFoundException::new(count).into()),
            }
        }

        /// Get the first related model, or call the callback when there is none.
        pub async fn first_or<F, Fut, E>(&self, callback: F) -> ::illuminate_support::Result<R>
        where
            F: FnOnce() -> Fut,
            Fut: ::std::future::Future<Output = ::std::result::Result<R, E>>,
            E: Into<::illuminate_support::Error>,
        {
            match self.first().await? {
                Some(model) => Ok(model),
                None => callback().await.map_err(Into::into),
            }
        }

        /// Stream the related models, loading them in chunks (ordered by
        /// primary key unless the relationship is ordered).
        pub fn lazy(
            &self,
            chunk_size: i64,
        ) -> ::futures::stream::BoxStream<'static, ::illuminate_support::Result<R>> {
            use ::futures::{StreamExt, TryStreamExt};
            let mut query = self.get_query();
            if query.get_query().orders.is_empty() {
                let key = query.qualify_column(R::primary_key());
                query = query.order_by(key, "asc");
            }
            let chunk_size = chunk_size.max(1);
            let relation = self.clone();
            ::futures::stream::try_unfold(Some((relation, query, 1_i64)), move |state| async move {
                let Some((relation, query, page)) = state else {
                    return Ok::<_, ::illuminate_support::Error>(None);
                };
                let models = relation
                    .fetch(query.clone().for_page(page, chunk_size))
                    .await?;
                if models.is_empty() {
                    return Ok(None);
                }
                let next =
                    (models.len() as i64 == chunk_size).then_some((relation, query, page + 1));
                Ok(Some((
                    ::futures::stream::iter(
                        models.into_iter().map(Ok::<R, ::illuminate_support::Error>),
                    ),
                    next,
                )))
            })
            .try_flatten()
            .boxed()
        }

        /// Stream the related models in chunks paginated by primary key.
        pub fn lazy_by_id(
            &self,
            chunk_size: i64,
        ) -> ::futures::stream::BoxStream<'static, ::illuminate_support::Result<R>> {
            self.lazy_by_key(chunk_size, false)
        }

        /// Stream the related models in chunks paginated by primary key,
        /// descending.
        pub fn lazy_by_id_desc(
            &self,
            chunk_size: i64,
        ) -> ::futures::stream::BoxStream<'static, ::illuminate_support::Result<R>> {
            self.lazy_by_key(chunk_size, true)
        }

        fn lazy_by_key(
            &self,
            chunk_size: i64,
            descending: bool,
        ) -> ::futures::stream::BoxStream<'static, ::illuminate_support::Result<R>> {
            use ::futures::{StreamExt, TryStreamExt};
            let query = self.get_query();
            let key = query.qualify_column(R::primary_key());
            let chunk_size = chunk_size.max(1);
            let relation = self.clone();
            let initial: Option<(Self, Option<::illuminate_support::Value>)> =
                Some((relation, None));
            ::futures::stream::try_unfold(initial, move |state| {
                let (query, key) = (query.clone(), key.clone());
                async move {
                    let Some((relation, last)) = state else {
                        return Ok::<_, ::illuminate_support::Error>(None);
                    };
                    let mut query = query
                        .reorder_by(key.clone(), if descending { "desc" } else { "asc" })
                        .limit(chunk_size);
                    if let Some(last) = &last {
                        query =
                            query.where_op(key, if descending { "<" } else { ">" }, last.clone());
                    }
                    let models = relation.fetch(query).await?;
                    if models.is_empty() {
                        return Ok(None);
                    }
                    let next = (models.len() as i64 == chunk_size)
                        .then(|| models.last().map(|model| (relation, Some(model.get_key()))))
                        .flatten();
                    Ok(Some((
                        ::futures::stream::iter(
                            models.into_iter().map(Ok::<R, ::illuminate_support::Error>),
                        ),
                        next,
                    )))
                }
            })
            .try_flatten()
            .boxed()
        }

        /// Stream the related models (in chunks of 1,000, keeping memory
        /// flat).
        pub fn cursor(
            &self,
        ) -> ::futures::stream::BoxStream<'static, ::illuminate_support::Result<R>> {
            self.lazy(1000)
        }

        /// Run the callback for each related model, loading them in chunks
        /// paginated by primary key; return `Ok(false)` to stop.
        pub async fn each_by_id<F, Fut>(
            &self,
            count: i64,
            mut callback: F,
        ) -> ::illuminate_support::Result<bool>
        where
            F: FnMut(R) -> Fut,
            Fut: ::std::future::Future<Output = ::illuminate_support::Result<bool>>,
        {
            use ::futures::StreamExt;
            let mut models = self.lazy_by_id(count);
            while let Some(model) = models.next().await {
                if !callback(model?).await? {
                    return Ok(false);
                }
            }
            Ok(true)
        }

        /// Map every related model, loading them in chunks, into a collection.
        pub async fn chunk_map<U, F, Fut>(
            &self,
            mut callback: F,
            count: i64,
        ) -> ::illuminate_support::Result<::illuminate_support::Collection<U>>
        where
            F: FnMut(R) -> Fut,
            Fut: ::std::future::Future<Output = ::illuminate_support::Result<U>>,
        {
            use ::futures::StreamExt;
            let mut models = self.lazy(count);
            let mut mapped = Vec::new();
            while let Some(model) = models.next().await {
                mapped.push(callback(model?).await?);
            }
            Ok(mapped.into())
        }

        /// Cursor paginate the related models (`None` uses the related
        /// model's `per_page`).
        pub async fn cursor_paginate(
            &self,
            per_page: impl Into<Option<u64>>,
            cursor: Option<$crate::pagination::Cursor>,
        ) -> ::illuminate_support::Result<$crate::pagination::CursorPaginator<R>> {
            let per_page = per_page.into().unwrap_or_else(R::per_page).max(1);
            let cursor = cursor.or_else(|| $crate::pagination::resolve_current_cursor("cursor"));
            let mut query = self.get_query().applied();
            let (base, parameters) = query
                .get_query()
                .clone()
                .prepare_cursor_pagination(per_page, cursor.as_ref())?;
            *query.get_query_mut() = base;
            let items = self.fetch(query).await?;
            Ok($crate::pagination::CursorPaginator::new(
                items,
                per_page,
                cursor,
                $crate::pagination::CursorPaginatorOptions {
                    path: ::illuminate_pagination::current_path(),
                    parameters,
                    ..Default::default()
                },
            ))
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
