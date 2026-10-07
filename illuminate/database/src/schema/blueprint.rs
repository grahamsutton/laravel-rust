//! Blueprints describe a table: its columns, indexes and foreign keys.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use illuminate_support::{Result, Str, Value};

use super::grammar::SchemaGrammar;
use super::state::TableState;
use crate::driver::Driver;
use crate::expression::{Expression, Ident};

static DEFAULT_STRING_LENGTH: AtomicU32 = AtomicU32::new(255);
static DEFAULT_MORPH_KEY_TYPE: Mutex<&'static str> = Mutex::new("int");

/// Set the default string length for migrations (`Schema::default_string_length(191)`).
pub fn set_default_string_length(length: u32) {
    DEFAULT_STRING_LENGTH.store(length, Ordering::SeqCst);
}

/// Get the default string length for migrations.
pub fn default_string_length() -> u32 {
    DEFAULT_STRING_LENGTH.load(Ordering::SeqCst)
}

/// Set the default morph key type (`int`, `uuid` or `ulid`).
pub fn set_default_morph_key_type(kind: &str) -> Result<()> {
    let kind: &'static str = match kind {
        "int" => "int",
        "uuid" => "uuid",
        "ulid" => "ulid",
        _ => anyhow::bail!(illuminate_support::error::InvalidArgumentException::new(
            "Morph key type must be 'int', 'uuid', or 'ulid'."
        )),
    };
    *DEFAULT_MORPH_KEY_TYPE.lock().unwrap() = kind;
    Ok(())
}

fn default_morph_key_type() -> &'static str {
    *DEFAULT_MORPH_KEY_TYPE.lock().unwrap()
}

/// The default precision of time columns (Laravel 11+ uses `0`); `u32::MAX`
/// stands for "no precision".
static DEFAULT_TIME_PRECISION: AtomicU32 = AtomicU32::new(0);

/// Set the default precision of time columns (`None` leaves the precision
/// to the database).
pub fn set_default_time_precision(precision: Option<u32>) {
    DEFAULT_TIME_PRECISION.store(precision.unwrap_or(u32::MAX), Ordering::SeqCst);
}

/// Get the default precision of time columns.
pub fn default_time_precision() -> Option<u32> {
    match DEFAULT_TIME_PRECISION.load(Ordering::SeqCst) {
        u32::MAX => None,
        precision => Some(precision),
    }
}

/// A column's default value: a literal, or a raw expression.
#[derive(Clone, Debug, PartialEq)]
pub enum ColumnDefault {
    /// A literal value (quoted in the DDL).
    Value(Value),
    /// A raw expression such as `CURRENT_TIMESTAMP`.
    Raw(Expression),
}

impl<T: Into<Value>> From<T> for ColumnDefault {
    fn from(value: T) -> Self {
        ColumnDefault::Value(value.into())
    }
}

impl From<Expression> for ColumnDefault {
    fn from(value: Expression) -> Self {
        ColumnDefault::Raw(value)
    }
}

/// Whether a column should get a (named) index of some kind.
#[derive(Clone, Debug, PartialEq)]
pub enum IndexFlag {
    /// Add the index with Laravel's generated name.
    Yes,
    /// Remove the index (when changing a column).
    No,
    /// Add the index with the given name.
    Named(String),
}

/// The attributes of a column definition.
#[derive(Clone, Debug, Default)]
pub struct ColumnAttributes {
    pub kind: String,
    pub name: String,
    pub length: Option<u32>,
    pub precision: Option<u32>,
    pub total: Option<u32>,
    pub places: Option<u32>,
    pub auto_increment: bool,
    pub unsigned: bool,
    pub nullable: Option<bool>,
    pub default: Option<ColumnDefault>,
    pub use_current: bool,
    pub use_current_on_update: bool,
    pub on_update: Option<Expression>,
    pub comment: Option<String>,
    pub after: Option<String>,
    pub first: bool,
    pub charset: Option<String>,
    pub collation: Option<String>,
    pub change: bool,
    pub virtual_as: Option<String>,
    pub stored_as: Option<String>,
    pub generated_as: Option<String>,
    pub always: bool,
    pub allowed: Vec<String>,
    pub fixed: bool,
    pub primary: Option<IndexFlag>,
    pub unique: Option<IndexFlag>,
    pub index: Option<IndexFlag>,
    pub fulltext: Option<IndexFlag>,
    pub starting_value: Option<i64>,
    pub invisible: bool,
    pub definition: Option<String>,
    pub full_type_definition: Option<String>,
    pub using: Option<String>,
    /// The dimensions of a `vector` column.
    pub dimensions: Option<u32>,
    /// The subtype of a spatial column (`point`, `polygon`, ...).
    pub subtype: Option<String>,
    /// The spatial reference system identifier of a spatial column.
    pub srid: Option<u32>,
    /// The expression of a `computed` column.
    pub expression: Option<String>,
    /// Whether a computed column is persisted (SQL Server).
    pub persisted: bool,
    /// Whether to use `algorithm=instant` (MySQL).
    pub instant: bool,
    /// The DDL lock mode (MySQL).
    pub lock: Option<String>,
    pub spatial_index: Option<IndexFlag>,
    pub vector_index: Option<IndexFlag>,
    /// The table referenced by `constrained()` (set by `foreign_id_for`).
    pub foreign_table: Option<String>,
    /// The column referenced by `constrained()` (set by `foreign_id_for`).
    pub foreign_column: Option<String>,
}

/// A column being added to (or changed on) a table. Modifiers chain:
///
/// ```
/// # use illuminate_database::schema::Blueprint;
/// let mut table = Blueprint::new("users");
/// table.string("email").nullable().unique();
/// table.integer("votes").unsigned().default(0).comment("The votes");
/// ```
#[derive(Clone, Debug)]
pub struct ColumnDefinition {
    attributes: Arc<Mutex<ColumnAttributes>>,
    blueprint: Weak<Shared>,
}

impl ColumnDefinition {
    /// A snapshot of the column's attributes.
    pub fn attributes(&self) -> ColumnAttributes {
        self.attributes.lock().unwrap().clone()
    }

    fn with(self, f: impl FnOnce(&mut ColumnAttributes)) -> Self {
        f(&mut self.attributes.lock().unwrap());
        self
    }

    /// The column's name.
    pub fn name(&self) -> String {
        self.attributes.lock().unwrap().name.clone()
    }

    /// The column's type (`string`, `bigInteger`, ...).
    pub fn kind(&self) -> String {
        self.attributes.lock().unwrap().kind.clone()
    }

    /// Allow NULL values to be inserted into the column.
    pub fn nullable(self) -> Self {
        self.with(|c| c.nullable = Some(true))
    }

    /// Set whether the column is nullable.
    pub fn nullable_if(self, value: bool) -> Self {
        self.with(|c| c.nullable = Some(value))
    }

    /// Specify a "default" value for the column (a value or a raw `Expression`).
    pub fn default(self, value: impl Into<ColumnDefault>) -> Self {
        let value = value.into();
        self.with(|c| c.default = Some(value))
    }

    /// Add a unique index on the column.
    pub fn unique(self) -> Self {
        self.with(|c| c.unique = Some(IndexFlag::Yes))
    }

    /// Add a unique index with the given name.
    pub fn unique_named(self, name: &str) -> Self {
        let name = name.to_string();
        self.with(|c| c.unique = Some(IndexFlag::Named(name)))
    }

    /// Add an index on the column.
    pub fn index(self) -> Self {
        self.with(|c| c.index = Some(IndexFlag::Yes))
    }

    /// Add an index with the given name.
    pub fn index_named(self, name: &str) -> Self {
        let name = name.to_string();
        self.with(|c| c.index = Some(IndexFlag::Named(name)))
    }

    /// Make the column the primary key.
    pub fn primary(self) -> Self {
        self.with(|c| c.primary = Some(IndexFlag::Yes))
    }

    /// Add a fulltext index on the column.
    pub fn fulltext(self) -> Self {
        self.with(|c| c.fulltext = Some(IndexFlag::Yes))
    }

    /// Drop the column's unique index (when changing a column).
    pub fn without_unique(self) -> Self {
        self.with(|c| c.unique = Some(IndexFlag::No))
    }

    /// Drop the column's index (when changing a column).
    pub fn without_index(self) -> Self {
        self.with(|c| c.index = Some(IndexFlag::No))
    }

    /// Set an INTEGER column as UNSIGNED (MySQL).
    pub fn unsigned(self) -> Self {
        self.with(|c| c.unsigned = true)
    }

    /// Set an INTEGER column as auto-increment (primary key).
    pub fn auto_increment(self) -> Self {
        self.with(|c| c.auto_increment = true)
    }

    /// Set the starting value of an auto-incrementing column (MySQL / PostgreSQL).
    pub fn starting_value(self, value: i64) -> Self {
        self.with(|c| c.starting_value = Some(value))
    }

    /// Alias of [`ColumnDefinition::starting_value`].
    pub fn from(self, value: i64) -> Self {
        self.starting_value(value)
    }

    /// Add a comment to the column (MySQL / PostgreSQL).
    pub fn comment(self, comment: &str) -> Self {
        let comment = comment.to_string();
        self.with(|c| c.comment = Some(comment))
    }

    /// Place the column "after" another column (MySQL).
    pub fn after(self, column: &str) -> Self {
        let column = column.to_string();
        self.with(|c| c.after = Some(column))
    }

    /// Place the column "first" in the table (MySQL).
    pub fn first(self) -> Self {
        self.with(|c| c.first = true)
    }

    /// Set a TIMESTAMP column to use CURRENT_TIMESTAMP as its default.
    pub fn use_current(self) -> Self {
        self.with(|c| c.use_current = true)
    }

    /// Set a TIMESTAMP column to use CURRENT_TIMESTAMP when updated (MySQL).
    pub fn use_current_on_update(self) -> Self {
        self.with(|c| c.use_current_on_update = true)
    }

    /// Change an existing column instead of adding it.
    pub fn change(self) -> Self {
        self.with(|c| c.change = true)
    }

    /// Specify a character set for the column (MySQL).
    pub fn charset(self, charset: &str) -> Self {
        let charset = charset.to_string();
        self.with(|c| c.charset = Some(charset))
    }

    /// Specify a collation for the column.
    pub fn collation(self, collation: &str) -> Self {
        let collation = collation.to_string();
        self.with(|c| c.collation = Some(collation))
    }

    /// Create a virtual generated column.
    pub fn virtual_as(self, expression: &str) -> Self {
        let expression = expression.to_string();
        self.with(|c| c.virtual_as = Some(expression))
    }

    /// Create a stored generated column.
    pub fn stored_as(self, expression: &str) -> Self {
        let expression = expression.to_string();
        self.with(|c| c.stored_as = Some(expression))
    }

    /// Create an SQL compliant identity column (PostgreSQL).
    pub fn generated_as(self, expression: &str) -> Self {
        let expression = expression.to_string();
        self.with(|c| c.generated_as = Some(expression))
    }

    /// Use `generated always` for an identity column (PostgreSQL).
    pub fn always(self) -> Self {
        self.with(|c| c.always = true)
    }

    /// Make the column invisible to `select *` (MySQL).
    pub fn invisible(self) -> Self {
        self.with(|c| c.invisible = true)
    }

    /// Set the length of a string / char / binary column.
    pub fn length(self, length: u32) -> Self {
        self.with(|c| c.length = Some(length))
    }

    /// Set the precision of a float or time column.
    pub fn precision(self, precision: u32) -> Self {
        self.with(|c| c.precision = Some(precision))
    }

    /// Make a binary column fixed length (MySQL `binary(n)`).
    pub fn fixed(self) -> Self {
        self.with(|c| c.fixed = true)
    }

    /// Specify a casting expression when changing the column type (PostgreSQL).
    pub fn using(self, expression: &str) -> Self {
        let expression = expression.to_string();
        self.with(|c| c.using = Some(expression))
    }

    /// Mark a computed column as persisted (SQL Server; other drivers ignore
    /// it).
    pub fn persisted(self) -> Self {
        self.with(|c| c.persisted = true)
    }

    /// Use `algorithm=instant` for the column operation (MySQL).
    pub fn instant(self) -> Self {
        self.with(|c| c.instant = true)
    }

    /// Specify the DDL lock mode for the column operation (MySQL): `none`,
    /// `shared`, `default` or `exclusive`.
    pub fn lock(self, mode: &str) -> Self {
        let mode = mode.to_string();
        self.with(|c| c.lock = Some(mode))
    }

    /// Specify the column's type (`string`, `text`, ...), usually with
    /// [`change`](ColumnDefinition::change).
    pub fn type_(self, kind: &str) -> Self {
        let kind = kind.to_string();
        self.with(|c| c.kind = kind)
    }

    /// Add a spatial index on the column.
    pub fn spatial_index(self) -> Self {
        self.with(|c| c.spatial_index = Some(IndexFlag::Yes))
    }

    /// Add a spatial index with the given name.
    pub fn spatial_index_named(self, name: &str) -> Self {
        let name = name.to_string();
        self.with(|c| c.spatial_index = Some(IndexFlag::Named(name)))
    }

    /// Add a vector index on the column.
    pub fn vector_index(self) -> Self {
        self.with(|c| c.vector_index = Some(IndexFlag::Yes))
    }

    /// Add a vector index with the given name.
    pub fn vector_index_named(self, name: &str) -> Self {
        let name = name.to_string();
        self.with(|c| c.vector_index = Some(IndexFlag::Named(name)))
    }

    /// Create a foreign key constraint on this column referencing the given
    /// column (`foreign_id("user_id").references("id").on("users")`).
    pub fn references(self, column: &str) -> ForeignKeyDefinition {
        let name = self.name();
        match self.blueprint.upgrade() {
            Some(shared) => shared.foreign(vec![name], None).references(column),
            None => ForeignKeyDefinition::detached(name).references(column),
        }
    }

    /// Create a foreign key constraint referencing `id` on the table guessed
    /// from the column name (`user_id` → `users`), or the model's key and
    /// table for `foreign_id_for` columns.
    pub fn constrained(self) -> ForeignKeyDefinition {
        let attributes = self.attributes();
        let table = attributes
            .foreign_table
            .unwrap_or_else(|| Str::plural(&Str::before_last(&attributes.name, "_id")));
        let column = attributes.foreign_column.unwrap_or_else(|| "id".into());
        self.references(&column).on(&table)
    }

    /// Create a foreign key constraint referencing `id` on the given table.
    pub fn constrained_on(self, table: &str) -> ForeignKeyDefinition {
        self.references("id").on(table)
    }

    /// Create a foreign key constraint referencing a column on a table.
    pub fn constrained_with(self, table: &str, column: &str) -> ForeignKeyDefinition {
        self.references(column).on(table)
    }
}

/// The attributes of a blueprint command.
#[derive(Clone, Debug, Default)]
pub struct CommandAttributes {
    pub name: String,
    pub index: Option<String>,
    pub columns: Vec<Ident>,
    pub algorithm: Option<String>,
    pub language: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub comment: Option<String>,
    pub column: Option<ColumnDefinition>,
    pub on: Option<String>,
    pub references: Vec<String>,
    pub on_delete: Option<String>,
    pub on_update: Option<String>,
    pub deferrable: Option<bool>,
    pub initially_immediate: Option<bool>,
    pub not_valid: bool,
    pub online: bool,
    pub should_be_skipped: bool,
    /// The operator class of a spatial or vector index (PostgreSQL).
    pub operator_class: Option<String>,
}

impl CommandAttributes {
    /// The command's columns as plain names.
    pub fn column_names(&self) -> Vec<String> {
        self.columns.iter().map(|c| c.value().to_string()).collect()
    }
}

/// A blueprint command (an index, a foreign key, a drop, ...).
#[derive(Clone, Debug)]
pub struct CommandDefinition {
    attributes: Arc<Mutex<CommandAttributes>>,
}

impl CommandDefinition {
    pub(crate) fn new(attributes: CommandAttributes) -> Self {
        Self {
            attributes: Arc::new(Mutex::new(attributes)),
        }
    }

    /// A snapshot of the command's attributes.
    pub fn attributes(&self) -> CommandAttributes {
        self.attributes.lock().unwrap().clone()
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, CommandAttributes> {
        self.attributes.lock().unwrap()
    }

    /// The command's name.
    pub fn name(&self) -> String {
        self.lock().name.clone()
    }

    fn with(self, f: impl FnOnce(&mut CommandAttributes)) -> Self {
        f(&mut self.lock());
        self
    }
}

/// An index command, returned by `primary`, `unique`, `index` and `fulltext`.
#[derive(Clone, Debug)]
pub struct IndexDefinition(pub(crate) CommandDefinition);

impl IndexDefinition {
    /// Specify an algorithm for the index (MySQL / PostgreSQL).
    pub fn algorithm(self, algorithm: &str) -> Self {
        let algorithm = algorithm.to_string();
        Self(self.0.with(|c| c.algorithm = Some(algorithm)))
    }

    /// Specify a language for a fulltext index (PostgreSQL).
    pub fn language(self, language: &str) -> Self {
        let language = language.to_string();
        Self(self.0.with(|c| c.language = Some(language)))
    }

    /// Create the index concurrently (PostgreSQL).
    pub fn online(self) -> Self {
        Self(self.0.with(|c| c.online = true))
    }

    /// Specify the operator class of a spatial or vector index (PostgreSQL),
    /// or the options of a MariaDB vector index.
    pub fn operator_class(self, operator_class: &str) -> Self {
        let operator_class = operator_class.to_string();
        Self(self.0.with(|c| c.operator_class = Some(operator_class)))
    }
}

/// A foreign key constraint.
///
/// ```
/// # use illuminate_database::schema::Blueprint;
/// let mut table = Blueprint::new("posts");
/// table.unsigned_big_integer("user_id");
/// table.foreign("user_id").references("id").on("users").cascade_on_delete();
/// ```
#[derive(Clone, Debug)]
pub struct ForeignKeyDefinition(pub(crate) CommandDefinition);

impl ForeignKeyDefinition {
    fn detached(column: String) -> Self {
        Self(CommandDefinition::new(CommandAttributes {
            name: "foreign".into(),
            columns: vec![Ident::Name(column)],
            ..Default::default()
        }))
    }

    fn with(self, f: impl FnOnce(&mut CommandAttributes)) -> Self {
        Self(self.0.with(f))
    }

    /// Specify the referenced column(s).
    pub fn references(self, columns: impl super::IntoColumnNames) -> Self {
        let columns = columns.into_column_names();
        self.with(|c| c.references = columns)
    }

    /// Specify the referenced table.
    pub fn on(self, table: &str) -> Self {
        let table = table.to_string();
        self.with(|c| c.on = Some(table))
    }

    /// Add an ON DELETE action.
    pub fn on_delete(self, action: &str) -> Self {
        let action = action.to_string();
        self.with(|c| c.on_delete = Some(action))
    }

    /// Add an ON UPDATE action.
    pub fn on_update(self, action: &str) -> Self {
        let action = action.to_string();
        self.with(|c| c.on_update = Some(action))
    }

    /// Indicate that updates should cascade.
    pub fn cascade_on_update(self) -> Self {
        self.on_update("cascade")
    }

    /// Indicate that updates should be restricted.
    pub fn restrict_on_update(self) -> Self {
        self.on_update("restrict")
    }

    /// Indicate that updates should set the foreign key value to null.
    pub fn null_on_update(self) -> Self {
        self.on_update("set null")
    }

    /// Indicate that updates should have "no action".
    pub fn no_action_on_update(self) -> Self {
        self.on_update("no action")
    }

    /// Indicate that deletes should cascade.
    pub fn cascade_on_delete(self) -> Self {
        self.on_delete("cascade")
    }

    /// Indicate that deletes should be restricted.
    pub fn restrict_on_delete(self) -> Self {
        self.on_delete("restrict")
    }

    /// Indicate that deletes should set the foreign key value to null.
    pub fn null_on_delete(self) -> Self {
        self.on_delete("set null")
    }

    /// Indicate that deletes should have "no action".
    pub fn no_action_on_delete(self) -> Self {
        self.on_delete("no action")
    }

    /// Set the foreign key as deferrable (PostgreSQL).
    pub fn deferrable(self) -> Self {
        self.with(|c| c.deferrable = Some(true))
    }

    /// Set the default time to check the constraint (PostgreSQL).
    pub fn initially_immediate(self) -> Self {
        self.with(|c| c.initially_immediate = Some(true))
    }

    /// Skip validation of existing rows (PostgreSQL).
    pub fn not_valid(self) -> Self {
        self.with(|c| c.not_valid = true)
    }

    /// A snapshot of the constraint's attributes.
    pub fn attributes(&self) -> CommandAttributes {
        self.0.attributes()
    }
}

/// A command queued on the blueprint: a column (when altering a table) or
/// a named command.
#[derive(Clone, Debug)]
pub enum Command {
    /// A column added to an existing table.
    Column(ColumnDefinition),
    /// Any other command.
    Command(CommandDefinition),
}

struct State {
    columns: Vec<ColumnDefinition>,
    commands: Vec<Command>,
    creating: bool,
}

/// The state shared between a blueprint and its column definitions.
pub(crate) struct Shared {
    table: String,
    index_prefix: String,
    state: Mutex<State>,
}

impl Shared {
    fn create_index_name(&self, kind: &str, columns: &[String]) -> String {
        let index = format!(
            "{}{}_{}_{kind}",
            self.index_prefix,
            self.table,
            columns.join("_")
        )
        .to_lowercase();
        index.replace(['-', '.'], "_")
    }

    fn add_command(&self, attributes: CommandAttributes) -> CommandDefinition {
        let command = CommandDefinition::new(attributes);
        self.state
            .lock()
            .unwrap()
            .commands
            .push(Command::Command(command.clone()));
        command
    }

    fn index_command(
        &self,
        kind: &str,
        columns: Vec<Ident>,
        name: Option<String>,
    ) -> CommandDefinition {
        let names: Vec<String> = columns.iter().map(|c| c.value().to_string()).collect();
        let index = name.unwrap_or_else(|| self.create_index_name(kind, &names));
        self.add_command(CommandAttributes {
            name: kind.to_string(),
            index: Some(index),
            columns,
            ..Default::default()
        })
    }

    fn foreign(&self, columns: Vec<String>, name: Option<String>) -> ForeignKeyDefinition {
        let columns = columns.into_iter().map(Ident::Name).collect();
        ForeignKeyDefinition(self.index_command("foreign", columns, name))
    }
}

/// A table blueprint, handed to the closures given to `Schema::create` and
/// `Schema::table`.
///
/// ```
/// use illuminate_database::schema::{Blueprint, SchemaGrammar};
/// use illuminate_database::Driver;
///
/// let mut table = Blueprint::creating("users");
/// table.id();
/// table.string("name");
/// table.string("email").unique();
/// table.timestamps();
///
/// let grammar = SchemaGrammar::new(Driver::Sqlite, "", Default::default());
/// assert_eq!(table.to_sql(&grammar).unwrap(), vec![
///     "create table \"users\" (\"id\" integer primary key autoincrement not null, \"name\" varchar not null, \"email\" varchar not null, \"created_at\" datetime, \"updated_at\" datetime)",
///     "create unique index \"users_email_unique\" on \"users\" (\"email\")",
/// ]);
/// ```
pub struct Blueprint {
    shared: Arc<Shared>,
    /// The storage engine for the table (MySQL).
    pub engine: Option<String>,
    /// The default character set for the table (MySQL).
    pub charset: Option<String>,
    /// The collation for the table (MySQL).
    pub collation: Option<String>,
    /// Whether the table is temporary.
    pub temporary: bool,
    after: Option<String>,
    implied: bool,
}

impl std::fmt::Debug for Blueprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Blueprint")
            .field("table", &self.shared.table)
            .finish()
    }
}

impl Blueprint {
    /// Create a blueprint for altering an existing table.
    pub fn new(table: &str) -> Self {
        Self::with_prefix(table, "")
    }

    /// Create a blueprint for a new table (with a `create` command).
    pub fn creating(table: &str) -> Self {
        let mut blueprint = Self::new(table);
        blueprint.create();
        blueprint
    }

    /// Create a blueprint whose generated index names use the given prefix.
    pub fn with_prefix(table: &str, index_prefix: &str) -> Self {
        Self {
            shared: Arc::new(Shared {
                table: table.to_string(),
                index_prefix: index_prefix.to_string(),
                state: Mutex::new(State {
                    columns: Vec::new(),
                    commands: Vec::new(),
                    creating: false,
                }),
            }),
            engine: None,
            charset: None,
            collation: None,
            temporary: false,
            after: None,
            implied: false,
        }
    }

    /// Get the table the blueprint describes.
    pub fn get_table(&self) -> &str {
        &self.shared.table
    }

    /// Get the columns on the blueprint.
    pub fn get_columns(&self) -> Vec<ColumnDefinition> {
        self.shared.state.lock().unwrap().columns.clone()
    }

    /// Get the columns that should be added.
    pub fn get_added_columns(&self) -> Vec<ColumnDefinition> {
        self.get_columns()
            .into_iter()
            .filter(|c| !c.attributes.lock().unwrap().change)
            .collect()
    }

    /// Get the columns that should be changed.
    pub fn get_changed_columns(&self) -> Vec<ColumnDefinition> {
        self.get_columns()
            .into_iter()
            .filter(|c| c.attributes.lock().unwrap().change)
            .collect()
    }

    /// Get the commands on the blueprint.
    pub fn get_commands(&self) -> Vec<Command> {
        self.shared.state.lock().unwrap().commands.clone()
    }

    pub(crate) fn command_definitions(&self) -> Vec<CommandDefinition> {
        self.get_commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::Command(c) => Some(c),
                Command::Column(_) => None,
            })
            .collect()
    }

    pub(crate) fn commands_named(&self, name: &str) -> Vec<CommandDefinition> {
        self.command_definitions()
            .into_iter()
            .filter(|c| c.lock().name == name)
            .collect()
    }

    pub(crate) fn has_command(&self, name: &str) -> bool {
        !self.commands_named(name).is_empty()
    }

    /// Determine if the blueprint has a create command.
    pub fn creating_table(&self) -> bool {
        self.shared.state.lock().unwrap().creating
    }

    // ------------------------------------------------------------------
    // Table commands
    // ------------------------------------------------------------------

    fn add_command(&mut self, attributes: CommandAttributes) -> CommandDefinition {
        self.shared.add_command(attributes)
    }

    fn named(&mut self, name: &str) -> CommandDefinition {
        self.add_command(CommandAttributes {
            name: name.to_string(),
            ..Default::default()
        })
    }

    /// Indicate that the table needs to be created.
    pub fn create(&mut self) {
        self.shared.state.lock().unwrap().creating = true;
        self.named("create");
    }

    /// Specify the storage engine for the table (MySQL).
    pub fn engine(&mut self, engine: &str) {
        self.engine = Some(engine.to_string());
    }

    /// Specify that the InnoDB storage engine should be used (MySQL).
    pub fn inno_db(&mut self) {
        self.engine("InnoDB");
    }

    /// Indicate that the table needs to be temporary.
    pub fn temporary(&mut self) {
        self.temporary = true;
    }

    /// Indicate that the table should be dropped.
    #[allow(clippy::should_implement_trait)]
    pub fn drop(&mut self) {
        self.named("drop");
    }

    /// Indicate that the table should be dropped if it exists.
    pub fn drop_if_exists(&mut self) {
        self.named("dropIfExists");
    }

    /// Rename the table to a given name.
    pub fn rename(&mut self, to: &str) {
        self.add_command(CommandAttributes {
            name: "rename".into(),
            to: Some(to.to_string()),
            ..Default::default()
        });
    }

    /// Add a comment to the table.
    pub fn comment(&mut self, comment: &str) {
        self.add_command(CommandAttributes {
            name: "tableComment".into(),
            comment: Some(comment.to_string()),
            ..Default::default()
        });
    }

    /// Indicate that the given columns should be dropped.
    pub fn drop_column(&mut self, columns: impl super::IntoColumnNames) {
        let columns = columns
            .into_column_names()
            .into_iter()
            .map(Ident::Name)
            .collect();
        self.add_command(CommandAttributes {
            name: "dropColumn".into(),
            columns,
            ..Default::default()
        });
    }

    /// Alias of [`Blueprint::drop_column`] for several columns.
    pub fn drop_columns(&mut self, columns: impl super::IntoColumnNames) {
        self.drop_column(columns);
    }

    /// Indicate that the given column should be renamed.
    pub fn rename_column(&mut self, from: &str, to: &str) {
        self.add_command(CommandAttributes {
            name: "renameColumn".into(),
            from: Some(from.to_string()),
            to: Some(to.to_string()),
            ..Default::default()
        });
    }

    fn drop_index_command(
        &mut self,
        command: &str,
        kind: &str,
        index: super::IndexName,
    ) -> CommandDefinition {
        let (columns, name) = match index {
            super::IndexName::Name(name) => (Vec::new(), name),
            super::IndexName::Columns(columns) => {
                let name = self.shared.create_index_name(kind, &columns);
                (columns, name)
            }
        };
        self.add_command(CommandAttributes {
            name: command.to_string(),
            index: Some(name),
            columns: columns.into_iter().map(Ident::Name).collect(),
            ..Default::default()
        })
    }

    /// Drop the primary key (by name, or by its columns).
    pub fn drop_primary(&mut self, index: impl Into<super::IndexName>) {
        self.drop_index_command("dropPrimary", "primary", index.into());
    }

    /// Drop a unique index (`"users_email_unique"`, or `vec!["email"]`).
    pub fn drop_unique(&mut self, index: impl Into<super::IndexName>) {
        self.drop_index_command("dropUnique", "unique", index.into());
    }

    /// Drop a basic index (by name, or by its columns).
    pub fn drop_index(&mut self, index: impl Into<super::IndexName>) {
        self.drop_index_command("dropIndex", "index", index.into());
    }

    /// Drop a fulltext index (by name, or by its columns).
    pub fn drop_fulltext(&mut self, index: impl Into<super::IndexName>) {
        self.drop_index_command("dropFullText", "fulltext", index.into());
    }

    /// Drop a fulltext index (Laravel's `dropFullText`).
    pub fn drop_full_text(&mut self, index: impl Into<super::IndexName>) {
        self.drop_fulltext(index);
    }

    /// Drop a spatial index (by name, or by its columns).
    pub fn drop_spatial_index(&mut self, index: impl Into<super::IndexName>) {
        self.drop_index_command("dropSpatialIndex", "spatialindex", index.into());
    }

    /// Drop a vector index (by name, or by its columns).
    pub fn drop_vector_index(&mut self, index: impl Into<super::IndexName>) {
        self.drop_index_command("dropVectorIndex", "vectorindex", index.into());
    }

    /// Drop the foreign key column for a model (`user_id` for `User`).
    pub fn drop_foreign_id_for<M: crate::eloquent::Model>(&mut self) {
        self.drop_column(M::get_foreign_key());
    }

    /// Drop a model's foreign key constraint and column.
    pub fn drop_constrained_foreign_id_for<M: crate::eloquent::Model>(&mut self) {
        self.drop_constrained_foreign_id(&M::get_foreign_key());
    }

    /// Drop a foreign key (`"posts_user_id_foreign"`, or `vec!["user_id"]`).
    pub fn drop_foreign(&mut self, index: impl Into<super::IndexName>) {
        self.drop_index_command("dropForeign", "foreign", index.into());
    }

    /// Drop a column's foreign key constraint and then the column itself.
    pub fn drop_constrained_foreign_id(&mut self, column: &str) {
        self.drop_foreign(vec![column.to_string()]);
        self.drop_column(column);
    }

    /// Rename an index.
    pub fn rename_index(&mut self, from: &str, to: &str) {
        self.add_command(CommandAttributes {
            name: "renameIndex".into(),
            from: Some(from.to_string()),
            to: Some(to.to_string()),
            ..Default::default()
        });
    }

    /// Drop the `created_at` and `updated_at` columns.
    pub fn drop_timestamps(&mut self) {
        self.drop_column(["created_at", "updated_at"]);
    }

    /// Drop the `created_at` and `updated_at` columns.
    pub fn drop_timestamps_tz(&mut self) {
        self.drop_timestamps();
    }

    /// Drop the `deleted_at` column.
    pub fn drop_soft_deletes(&mut self) {
        self.drop_column("deleted_at");
    }

    /// Drop the `deleted_at` column (with time zone).
    pub fn drop_soft_deletes_tz(&mut self) {
        self.drop_soft_deletes();
    }

    /// Drop the `remember_token` column.
    pub fn drop_remember_token(&mut self) {
        self.drop_column("remember_token");
    }

    /// Drop a polymorphic relation's columns and index.
    pub fn drop_morphs(&mut self, name: &str) {
        self.drop_index(vec![format!("{name}_type"), format!("{name}_id")]);
        self.drop_column([format!("{name}_type"), format!("{name}_id")]);
    }

    // ------------------------------------------------------------------
    // Indexes
    // ------------------------------------------------------------------

    /// Specify the primary key(s) for the table.
    pub fn primary(&mut self, columns: impl super::IntoColumnNames) -> IndexDefinition {
        let columns = columns
            .into_column_names()
            .into_iter()
            .map(Ident::Name)
            .collect();
        IndexDefinition(self.shared.index_command("primary", columns, None))
    }

    /// Specify a unique index for the table.
    pub fn unique(&mut self, columns: impl super::IntoColumnNames) -> IndexDefinition {
        let columns = columns
            .into_column_names()
            .into_iter()
            .map(Ident::Name)
            .collect();
        IndexDefinition(self.shared.index_command("unique", columns, None))
    }

    /// Specify a named unique index for the table.
    pub fn unique_named(
        &mut self,
        columns: impl super::IntoColumnNames,
        name: &str,
    ) -> IndexDefinition {
        let columns = columns
            .into_column_names()
            .into_iter()
            .map(Ident::Name)
            .collect();
        IndexDefinition(
            self.shared
                .index_command("unique", columns, Some(name.to_string())),
        )
    }

    /// Specify an index for the table.
    pub fn index(&mut self, columns: impl super::IntoColumnNames) -> IndexDefinition {
        let columns = columns
            .into_column_names()
            .into_iter()
            .map(Ident::Name)
            .collect();
        IndexDefinition(self.shared.index_command("index", columns, None))
    }

    /// Specify a named index for the table.
    pub fn index_named(
        &mut self,
        columns: impl super::IntoColumnNames,
        name: &str,
    ) -> IndexDefinition {
        let columns = columns
            .into_column_names()
            .into_iter()
            .map(Ident::Name)
            .collect();
        IndexDefinition(
            self.shared
                .index_command("index", columns, Some(name.to_string())),
        )
    }

    /// Specify a fulltext index for the table (MySQL / PostgreSQL).
    pub fn fulltext(&mut self, columns: impl super::IntoColumnNames) -> IndexDefinition {
        let columns = columns
            .into_column_names()
            .into_iter()
            .map(Ident::Name)
            .collect();
        IndexDefinition(self.shared.index_command("fulltext", columns, None))
    }

    /// Specify a fulltext index for the table (Laravel's `fullText`).
    pub fn full_text(&mut self, columns: impl super::IntoColumnNames) -> IndexDefinition {
        self.fulltext(columns)
    }

    /// Specify a spatial index for the table (MySQL / PostgreSQL).
    pub fn spatial_index(&mut self, columns: impl super::IntoColumnNames) -> IndexDefinition {
        let columns = columns
            .into_column_names()
            .into_iter()
            .map(Ident::Name)
            .collect();
        IndexDefinition(self.shared.index_command("spatialIndex", columns, None))
    }

    /// Specify a vector index for the column (PostgreSQL `hnsw` with
    /// `vector_cosine_ops`, or MariaDB `M=6 DISTANCE=cosine`).
    pub fn vector_index(&mut self, column: &str) -> IndexDefinition {
        IndexDefinition(self.shared.index_command(
            "vectorIndex",
            vec![Ident::Name(column.to_string())],
            None,
        ))
    }

    /// Specify a named vector index for the column.
    pub fn vector_index_named(&mut self, column: &str, name: &str) -> IndexDefinition {
        IndexDefinition(self.shared.index_command(
            "vectorIndex",
            vec![Ident::Name(column.to_string())],
            Some(name.to_string()),
        ))
    }

    /// Specify a raw index for the table.
    pub fn raw_index(&mut self, expression: &str, name: &str) -> IndexDefinition {
        IndexDefinition(self.shared.index_command(
            "index",
            vec![Ident::Raw(Expression::new(expression))],
            Some(name.to_string()),
        ))
    }

    /// Specify a foreign key for the table.
    pub fn foreign(&mut self, columns: impl super::IntoColumnNames) -> ForeignKeyDefinition {
        self.shared.foreign(columns.into_column_names(), None)
    }

    /// Specify a named foreign key for the table.
    pub fn foreign_named(
        &mut self,
        columns: impl super::IntoColumnNames,
        name: &str,
    ) -> ForeignKeyDefinition {
        self.shared
            .foreign(columns.into_column_names(), Some(name.to_string()))
    }

    // ------------------------------------------------------------------
    // Columns
    // ------------------------------------------------------------------

    /// Add a new column to the blueprint.
    pub fn add_column(
        &mut self,
        kind: &str,
        name: &str,
        configure: impl FnOnce(&mut ColumnAttributes),
    ) -> ColumnDefinition {
        let mut attributes = ColumnAttributes {
            kind: kind.to_string(),
            name: name.to_string(),
            ..Default::default()
        };
        configure(&mut attributes);
        if let Some(after) = &self.after {
            attributes.after = Some(after.clone());
        }
        let definition = ColumnDefinition {
            attributes: Arc::new(Mutex::new(attributes)),
            blueprint: Arc::downgrade(&self.shared),
        };
        {
            let mut state = self.shared.state.lock().unwrap();
            state.columns.push(definition.clone());
            if !state.creating {
                state.commands.push(Command::Column(definition.clone()));
            }
        }
        if self.after.is_some() {
            self.after = Some(name.to_string());
        }
        definition
    }

    /// Add the columns created in the callback after the given column (MySQL).
    pub fn after(&mut self, column: &str, callback: impl FnOnce(&mut Blueprint)) {
        self.after = Some(column.to_string());
        callback(self);
        self.after = None;
    }

    /// Remove a column from the blueprint.
    pub fn remove_column(&mut self, name: &str) {
        let mut state = self.shared.state.lock().unwrap();
        state.columns.retain(|c| c.name() != name);
        state.commands.retain(|c| match c {
            Command::Column(column) => column.name() != name,
            Command::Command(_) => true,
        });
    }

    /// Create a new auto-incrementing big integer `id` primary key.
    pub fn id(&mut self) -> ColumnDefinition {
        self.big_increments("id")
    }

    /// Create a new auto-incrementing big integer primary key with the given name.
    pub fn id_named(&mut self, column: &str) -> ColumnDefinition {
        self.big_increments(column)
    }

    /// Create a new auto-incrementing integer (4-byte) column.
    pub fn increments(&mut self, column: &str) -> ColumnDefinition {
        self.integer_column("integer", column, true, true)
    }

    /// Create a new auto-incrementing integer (4-byte) column.
    pub fn integer_increments(&mut self, column: &str) -> ColumnDefinition {
        self.increments(column)
    }

    /// Create a new auto-incrementing tiny integer (1-byte) column.
    pub fn tiny_increments(&mut self, column: &str) -> ColumnDefinition {
        self.integer_column("tinyInteger", column, true, true)
    }

    /// Create a new auto-incrementing small integer (2-byte) column.
    pub fn small_increments(&mut self, column: &str) -> ColumnDefinition {
        self.integer_column("smallInteger", column, true, true)
    }

    /// Create a new auto-incrementing medium integer (3-byte) column.
    pub fn medium_increments(&mut self, column: &str) -> ColumnDefinition {
        self.integer_column("mediumInteger", column, true, true)
    }

    /// Create a new auto-incrementing big integer (8-byte) column.
    pub fn big_increments(&mut self, column: &str) -> ColumnDefinition {
        self.integer_column("bigInteger", column, true, true)
    }

    fn integer_column(
        &mut self,
        kind: &str,
        column: &str,
        auto_increment: bool,
        unsigned: bool,
    ) -> ColumnDefinition {
        self.add_column(kind, column, |c| {
            c.auto_increment = auto_increment;
            c.unsigned = unsigned;
        })
    }

    /// Create a new char column (default length).
    pub fn char(&mut self, column: &str) -> ColumnDefinition {
        self.char_len(column, default_string_length())
    }

    /// Create a new char column with the given length.
    pub fn char_len(&mut self, column: &str, length: u32) -> ColumnDefinition {
        self.add_column("char", column, |c| c.length = Some(length))
    }

    /// Create a new string column (`varchar`, default length 255).
    pub fn string(&mut self, column: &str) -> ColumnDefinition {
        self.string_len(column, default_string_length())
    }

    /// Create a new string column with the given length.
    pub fn string_len(&mut self, column: &str, length: u32) -> ColumnDefinition {
        self.add_column("string", column, |c| c.length = Some(length))
    }

    /// Create a new tiny text column.
    pub fn tiny_text(&mut self, column: &str) -> ColumnDefinition {
        self.add_column("tinyText", column, |_| {})
    }

    /// Create a new text column.
    pub fn text(&mut self, column: &str) -> ColumnDefinition {
        self.add_column("text", column, |_| {})
    }

    /// Create a new medium text column.
    pub fn medium_text(&mut self, column: &str) -> ColumnDefinition {
        self.add_column("mediumText", column, |_| {})
    }

    /// Create a new long text column.
    pub fn long_text(&mut self, column: &str) -> ColumnDefinition {
        self.add_column("longText", column, |_| {})
    }

    /// Create a new integer (4-byte) column.
    pub fn integer(&mut self, column: &str) -> ColumnDefinition {
        self.integer_column("integer", column, false, false)
    }

    /// Create a new tiny integer (1-byte) column.
    pub fn tiny_integer(&mut self, column: &str) -> ColumnDefinition {
        self.integer_column("tinyInteger", column, false, false)
    }

    /// Create a new small integer (2-byte) column.
    pub fn small_integer(&mut self, column: &str) -> ColumnDefinition {
        self.integer_column("smallInteger", column, false, false)
    }

    /// Create a new medium integer (3-byte) column.
    pub fn medium_integer(&mut self, column: &str) -> ColumnDefinition {
        self.integer_column("mediumInteger", column, false, false)
    }

    /// Create a new big integer (8-byte) column.
    pub fn big_integer(&mut self, column: &str) -> ColumnDefinition {
        self.integer_column("bigInteger", column, false, false)
    }

    /// Create a new unsigned integer (4-byte) column.
    pub fn unsigned_integer(&mut self, column: &str) -> ColumnDefinition {
        self.integer_column("integer", column, false, true)
    }

    /// Create a new unsigned tiny integer (1-byte) column.
    pub fn unsigned_tiny_integer(&mut self, column: &str) -> ColumnDefinition {
        self.integer_column("tinyInteger", column, false, true)
    }

    /// Create a new unsigned small integer (2-byte) column.
    pub fn unsigned_small_integer(&mut self, column: &str) -> ColumnDefinition {
        self.integer_column("smallInteger", column, false, true)
    }

    /// Create a new unsigned medium integer (3-byte) column.
    pub fn unsigned_medium_integer(&mut self, column: &str) -> ColumnDefinition {
        self.integer_column("mediumInteger", column, false, true)
    }

    /// Create a new unsigned big integer (8-byte) column.
    pub fn unsigned_big_integer(&mut self, column: &str) -> ColumnDefinition {
        self.integer_column("bigInteger", column, false, true)
    }

    /// Create a new unsigned big integer column for a foreign key
    /// (`foreign_id("user_id").constrained()`).
    pub fn foreign_id(&mut self, column: &str) -> ColumnDefinition {
        self.integer_column("bigInteger", column, false, true)
    }

    /// Create a new UUID column for a foreign key.
    pub fn foreign_uuid(&mut self, column: &str) -> ColumnDefinition {
        self.add_column("uuid", column, |_| {})
    }

    /// Create a new ULID column for a foreign key.
    pub fn foreign_ulid(&mut self, column: &str) -> ColumnDefinition {
        self.add_column("char", column, |c| c.length = Some(26))
    }

    /// Create a foreign key column for a model (`user_id` for `User`): an
    /// unsigned big integer, a ULID or a UUID depending on the model's key.
    /// `constrained()` references the model's table and key.
    ///
    /// ```ignore
    /// table.foreign_id_for::<User>().constrained().cascade_on_delete();
    /// ```
    pub fn foreign_id_for<M: crate::eloquent::Model>(&mut self) -> ColumnDefinition {
        self.foreign_id_for_column::<M>(&M::get_foreign_key())
    }

    /// Create a foreign key column for a model, with the given column name.
    pub fn foreign_id_for_column<M: crate::eloquent::Model>(
        &mut self,
        column: &str,
    ) -> ColumnDefinition {
        let definition = match (M::key_type(), M::unique_ids()) {
            (crate::eloquent::KeyType::Int, _) => self.foreign_id(column),
            (_, crate::eloquent::UniqueIds::Ulid) => self.foreign_ulid(column),
            _ => self.foreign_uuid(column),
        };
        Self::references_model::<M>(definition)
    }

    /// Create a UUID foreign key column for a model.
    pub fn foreign_uuid_for<M: crate::eloquent::Model>(&mut self) -> ColumnDefinition {
        let definition = self.foreign_uuid(&M::get_foreign_key());
        Self::references_model::<M>(definition)
    }

    /// Create a ULID foreign key column for a model.
    pub fn foreign_ulid_for<M: crate::eloquent::Model>(&mut self) -> ColumnDefinition {
        let definition = self.foreign_ulid(&M::get_foreign_key());
        Self::references_model::<M>(definition)
    }

    fn references_model<M: crate::eloquent::Model>(
        definition: ColumnDefinition,
    ) -> ColumnDefinition {
        definition.with(|c| {
            c.foreign_table = Some(M::table());
            c.foreign_column = Some(M::primary_key().to_string());
        })
    }

    /// Create a new float column (precision 53).
    pub fn float(&mut self, column: &str) -> ColumnDefinition {
        self.add_column("float", column, |c| c.precision = Some(53))
    }

    /// Create a new double column.
    pub fn double(&mut self, column: &str) -> ColumnDefinition {
        self.add_column("double", column, |_| {})
    }

    /// Create a new decimal column with the given total digits and places.
    pub fn decimal(&mut self, column: &str, total: u32, places: u32) -> ColumnDefinition {
        self.add_column("decimal", column, |c| {
            c.total = Some(total);
            c.places = Some(places);
        })
    }

    /// Create a new boolean column.
    pub fn boolean(&mut self, column: &str) -> ColumnDefinition {
        self.add_column("boolean", column, |_| {})
    }

    /// Create a new enum column with the allowed values.
    pub fn enum_(
        &mut self,
        column: &str,
        allowed: impl super::IntoColumnNames,
    ) -> ColumnDefinition {
        let allowed = allowed.into_column_names();
        self.add_column("enum", column, |c| c.allowed = allowed)
    }

    /// Create a new set column (MySQL) with the allowed values.
    pub fn set(&mut self, column: &str, allowed: impl super::IntoColumnNames) -> ColumnDefinition {
        let allowed = allowed.into_column_names();
        self.add_column("set", column, |c| c.allowed = allowed)
    }

    /// Create a new json column.
    pub fn json(&mut self, column: &str) -> ColumnDefinition {
        self.add_column("json", column, |_| {})
    }

    /// Create a new jsonb column.
    pub fn jsonb(&mut self, column: &str) -> ColumnDefinition {
        self.add_column("jsonb", column, |_| {})
    }

    /// Create a new date column.
    pub fn date(&mut self, column: &str) -> ColumnDefinition {
        self.add_column("date", column, |_| {})
    }

    fn time_column(&mut self, kind: &str, column: &str) -> ColumnDefinition {
        self.add_column(kind, column, |c| c.precision = default_time_precision())
    }

    /// Create a new date-time column.
    pub fn date_time(&mut self, column: &str) -> ColumnDefinition {
        self.time_column("dateTime", column)
    }

    /// Create a new date-time column (with time zone).
    pub fn date_time_tz(&mut self, column: &str) -> ColumnDefinition {
        self.time_column("dateTimeTz", column)
    }

    /// Create a new time column.
    pub fn time(&mut self, column: &str) -> ColumnDefinition {
        self.time_column("time", column)
    }

    /// Create a new time column (with time zone).
    pub fn time_tz(&mut self, column: &str) -> ColumnDefinition {
        self.time_column("timeTz", column)
    }

    /// Create a new timestamp column.
    pub fn timestamp(&mut self, column: &str) -> ColumnDefinition {
        self.time_column("timestamp", column)
    }

    /// Create a new timestamp (with time zone) column.
    pub fn timestamp_tz(&mut self, column: &str) -> ColumnDefinition {
        self.time_column("timestampTz", column)
    }

    /// Add nullable `created_at` and `updated_at` timestamps to the table.
    pub fn timestamps(&mut self) {
        self.timestamp("created_at").nullable();
        self.timestamp("updated_at").nullable();
    }

    /// Alias of [`Blueprint::timestamps`].
    pub fn nullable_timestamps(&mut self) {
        self.timestamps();
    }

    /// Add nullable creation and update timestamps (with time zone).
    pub fn timestamps_tz(&mut self) {
        self.timestamp_tz("created_at").nullable();
        self.timestamp_tz("updated_at").nullable();
    }

    /// Alias of [`Blueprint::timestamps_tz`].
    pub fn nullable_timestamps_tz(&mut self) {
        self.timestamps_tz();
    }

    /// Add nullable `created_at` and `updated_at` date-time columns.
    pub fn datetimes(&mut self) {
        self.date_time("created_at").nullable();
        self.date_time("updated_at").nullable();
    }

    /// Add a nullable `deleted_at` timestamp for soft deletes.
    pub fn soft_deletes(&mut self) -> ColumnDefinition {
        self.timestamp("deleted_at").nullable()
    }

    /// Add a nullable soft delete timestamp with the given column name.
    pub fn soft_deletes_named(&mut self, column: &str) -> ColumnDefinition {
        self.timestamp(column).nullable()
    }

    /// Add a nullable `deleted_at` date-time column for soft deletes.
    pub fn soft_deletes_datetime(&mut self) -> ColumnDefinition {
        self.date_time("deleted_at").nullable()
    }

    /// Add a nullable soft delete date-time column with the given name.
    pub fn soft_deletes_datetime_named(&mut self, column: &str) -> ColumnDefinition {
        self.date_time(column).nullable()
    }

    /// Add a nullable `deleted_at` timestamp (with time zone) for soft deletes.
    pub fn soft_deletes_tz(&mut self) -> ColumnDefinition {
        self.timestamp_tz("deleted_at").nullable()
    }

    /// Create a new year column.
    pub fn year(&mut self, column: &str) -> ColumnDefinition {
        self.add_column("year", column, |_| {})
    }

    /// Create a new binary column.
    pub fn binary(&mut self, column: &str) -> ColumnDefinition {
        self.add_column("binary", column, |_| {})
    }

    /// Create a new UUID column.
    pub fn uuid(&mut self, column: &str) -> ColumnDefinition {
        self.add_column("uuid", column, |_| {})
    }

    /// Create a new ULID column (a 26 character `char`).
    pub fn ulid(&mut self, column: &str) -> ColumnDefinition {
        self.char_len(column, 26)
    }

    /// Create a new IP address column.
    pub fn ip_address(&mut self, column: &str) -> ColumnDefinition {
        self.add_column("ipAddress", column, |_| {})
    }

    /// Create a new MAC address column.
    pub fn mac_address(&mut self, column: &str) -> ColumnDefinition {
        self.add_column("macAddress", column, |_| {})
    }

    /// Add a nullable `remember_token` string column (length 100).
    pub fn remember_token(&mut self) -> ColumnDefinition {
        self.string_len("remember_token", 100).nullable()
    }

    /// Create a new vector column, optionally with its dimensions
    /// (PostgreSQL `pgvector`, MySQL 9 and MariaDB 11.7+).
    pub fn vector(&mut self, column: &str, dimensions: Option<u32>) -> ColumnDefinition {
        self.add_column("vector", column, |c| c.dimensions = dimensions)
    }

    /// Create a new geometry column (any subtype, SRID 0).
    pub fn geometry(&mut self, column: &str) -> ColumnDefinition {
        self.geometry_with(column, None, 0)
    }

    /// Create a new geometry column with a subtype (`point`, `polygon`, ...)
    /// and a spatial reference system identifier.
    pub fn geometry_with(
        &mut self,
        column: &str,
        subtype: Option<&str>,
        srid: u32,
    ) -> ColumnDefinition {
        let subtype = subtype.map(String::from);
        self.add_column("geometry", column, |c| {
            c.subtype = subtype;
            c.srid = Some(srid);
        })
    }

    /// Create a new geography column (any subtype, SRID 4326).
    pub fn geography(&mut self, column: &str) -> ColumnDefinition {
        self.geography_with(column, None, 4326)
    }

    /// Create a new geography column with a subtype and a spatial reference
    /// system identifier.
    pub fn geography_with(
        &mut self,
        column: &str,
        subtype: Option<&str>,
        srid: u32,
    ) -> ColumnDefinition {
        let subtype = subtype.map(String::from);
        self.add_column("geography", column, |c| {
            c.subtype = subtype;
            c.srid = Some(srid);
        })
    }

    /// Create a new generated, computed column (SQL Server). Other drivers
    /// use `virtual_as` / `stored_as` on a typed column instead.
    pub fn computed(&mut self, column: &str, expression: &str) -> ColumnDefinition {
        let expression = expression.to_string();
        self.add_column("computed", column, |c| c.expression = Some(expression))
    }

    /// Create a new `tsvector` column (PostgreSQL).
    pub fn tsvector(&mut self, column: &str) -> ColumnDefinition {
        self.add_column("tsvector", column, |_| {})
    }

    /// Create a new column with a raw definition.
    pub fn raw_column(&mut self, column: &str, definition: &str) -> ColumnDefinition {
        let definition = definition.to_string();
        self.add_column("raw", column, |c| c.definition = Some(definition))
    }

    /// Add the `{name}_type` and `{name}_id` columns (and an index) for a
    /// polymorphic relation, using the default morph key type.
    pub fn morphs(&mut self, name: &str) {
        match default_morph_key_type() {
            "uuid" => self.uuid_morphs(name),
            "ulid" => self.ulid_morphs(name),
            _ => self.numeric_morphs(name),
        }
    }

    /// Add nullable polymorphic relation columns.
    pub fn nullable_morphs(&mut self, name: &str) {
        match default_morph_key_type() {
            "uuid" => self.nullable_uuid_morphs(name),
            "ulid" => self.nullable_ulid_morphs(name),
            _ => self.nullable_numeric_morphs(name),
        }
    }

    fn morph_columns(&mut self, name: &str, id: &str, nullable: bool) {
        let kind = self.string(&format!("{name}_type"));
        let id_column = match id {
            "uuid" => self.uuid(&format!("{name}_id")),
            "ulid" => self.ulid(&format!("{name}_id")),
            _ => self.unsigned_big_integer(&format!("{name}_id")),
        };
        if nullable {
            kind.nullable();
            id_column.nullable();
        }
        self.index(vec![format!("{name}_type"), format!("{name}_id")]);
    }

    /// Add polymorphic relation columns with an integer id.
    pub fn numeric_morphs(&mut self, name: &str) {
        self.morph_columns(name, "int", false);
    }

    /// Add nullable polymorphic relation columns with an integer id.
    pub fn nullable_numeric_morphs(&mut self, name: &str) {
        self.morph_columns(name, "int", true);
    }

    /// Add polymorphic relation columns with a UUID id.
    pub fn uuid_morphs(&mut self, name: &str) {
        self.morph_columns(name, "uuid", false);
    }

    /// Add nullable polymorphic relation columns with a UUID id.
    pub fn nullable_uuid_morphs(&mut self, name: &str) {
        self.morph_columns(name, "uuid", true);
    }

    /// Add polymorphic relation columns with a ULID id.
    pub fn ulid_morphs(&mut self, name: &str) {
        self.morph_columns(name, "ulid", false);
    }

    /// Add nullable polymorphic relation columns with a ULID id.
    pub fn nullable_ulid_morphs(&mut self, name: &str) {
        self.morph_columns(name, "ulid", true);
    }

    // ------------------------------------------------------------------
    // Compiling
    // ------------------------------------------------------------------

    /// Add the commands implied by the blueprint's columns (fluent indexes,
    /// add / change commands, and SQLite's table rebuilds).
    pub(crate) fn add_implied_commands(&mut self, grammar: &SchemaGrammar) {
        if self.implied {
            return;
        }
        self.implied = true;
        self.add_fluent_indexes(grammar);
        self.add_fluent_commands(grammar);

        if !self.creating_table() {
            let mut state = self.shared.state.lock().unwrap();
            state.commands = std::mem::take(&mut state.commands)
                .into_iter()
                .map(|command| match command {
                    Command::Column(column) => {
                        let name = if column.attributes.lock().unwrap().change {
                            "change"
                        } else {
                            "add"
                        };
                        Command::Command(CommandDefinition::new(CommandAttributes {
                            name: name.into(),
                            column: Some(column),
                            ..Default::default()
                        }))
                    }
                    other => other,
                })
                .collect();
            drop(state);
            if grammar.driver() == Driver::Sqlite {
                self.add_alter_commands();
            }
        }
    }

    fn add_fluent_indexes(&mut self, grammar: &SchemaGrammar) {
        for column in self.get_columns() {
            let attributes = column.attributes();
            for kind in [
                "primary",
                "unique",
                "index",
                "fulltext",
                "spatialIndex",
                "vectorIndex",
            ] {
                let flag = match kind {
                    "primary" => attributes.primary.clone(),
                    "unique" => attributes.unique.clone(),
                    "index" => attributes.index.clone(),
                    "spatialIndex" => attributes.spatial_index.clone(),
                    "vectorIndex" => attributes.vector_index.clone(),
                    _ => attributes.fulltext.clone(),
                };
                let Some(flag) = flag else { continue };
                if kind == "primary"
                    && attributes.auto_increment
                    && attributes.change
                    && grammar.driver().is_mysql_family()
                {
                    break;
                }
                let columns = vec![Ident::Name(attributes.name.clone())];
                match flag {
                    IndexFlag::Yes => {
                        self.shared.index_command(kind, columns, None);
                    }
                    IndexFlag::Named(name) => {
                        self.shared.index_command(kind, columns, Some(name));
                    }
                    IndexFlag::No if attributes.change => {
                        let command = match kind {
                            "primary" => "dropPrimary",
                            "unique" => "dropUnique",
                            "index" => "dropIndex",
                            "spatialIndex" => "dropSpatialIndex",
                            "vectorIndex" => "dropVectorIndex",
                            _ => "dropFullText",
                        };
                        self.drop_index_command(
                            command,
                            kind,
                            super::IndexName::Columns(vec![attributes.name.clone()]),
                        );
                    }
                    IndexFlag::No => continue,
                }
                let mut guard = column.attributes.lock().unwrap();
                match kind {
                    "primary" => guard.primary = None,
                    "unique" => guard.unique = None,
                    "index" => guard.index = None,
                    "spatialIndex" => guard.spatial_index = None,
                    "vectorIndex" => guard.vector_index = None,
                    _ => guard.fulltext = None,
                }
                break;
            }
        }
    }

    fn add_fluent_commands(&mut self, grammar: &SchemaGrammar) {
        for column in self.get_columns() {
            for name in grammar.fluent_commands() {
                self.add_command(CommandAttributes {
                    name: name.to_string(),
                    column: Some(column.clone()),
                    ..Default::default()
                });
            }
        }
    }

    /// SQLite can't alter most things in place: queue `alter` commands that
    /// rebuild the table after each run of such commands.
    fn add_alter_commands(&mut self) {
        const ALTER: [&str; 5] = ["change", "primary", "dropPrimary", "foreign", "dropForeign"];
        let mut state = self.shared.state.lock().unwrap();
        let mut commands = Vec::new();
        let mut last_was_alter = false;
        for command in std::mem::take(&mut state.commands) {
            let name = match &command {
                Command::Command(c) => c.lock().name.clone(),
                Command::Column(_) => String::new(),
            };
            if ALTER.contains(&name.as_str()) {
                last_was_alter = true;
            } else if last_was_alter {
                commands.push(Command::Command(CommandDefinition::new(
                    CommandAttributes {
                        name: "alter".into(),
                        ..Default::default()
                    },
                )));
                last_was_alter = false;
            }
            commands.push(command);
        }
        if last_was_alter {
            commands.push(Command::Command(CommandDefinition::new(
                CommandAttributes {
                    name: "alter".into(),
                    ..Default::default()
                },
            )));
        }
        state.commands = commands;
    }

    /// Whether compiling the blueprint requires the table's current state
    /// (SQLite table rebuilds and index renames).
    pub(crate) fn needs_state(&self, grammar: &SchemaGrammar) -> bool {
        grammar.driver() == Driver::Sqlite
            && (self.has_command("alter") || self.has_command("renameIndex"))
    }

    /// Get the raw SQL statements for the blueprint.
    pub fn to_sql(&mut self, grammar: &SchemaGrammar) -> Result<Vec<String>> {
        self.compile_with_state(grammar, None)
    }

    pub(crate) fn compile_with_state(
        &mut self,
        grammar: &SchemaGrammar,
        mut state: Option<TableState>,
    ) -> Result<Vec<String>> {
        self.add_implied_commands(grammar);
        if self.needs_state(grammar) && state.is_none() {
            anyhow::bail!(crate::error::UnsupportedOperation(
                "This SQLite schema change requires inspecting the table; run it through the schema builder.".into()
            ));
        }
        let mut statements = Vec::new();
        for command in self.command_definitions() {
            if command.lock().should_be_skipped {
                continue;
            }
            if let Some(state) = state.as_mut() {
                state.update(&command.attributes());
            }
            statements.extend(grammar.compile_command(self, &command, state.as_ref())?);
        }
        Ok(statements)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_names_follow_laravels_convention() {
        let mut table = Blueprint::new("users");
        table.unique("email");
        table.index(["account_id", "created_at"]);
        table.foreign("user_id");
        let names: Vec<String> = table
            .command_definitions()
            .iter()
            .map(|c| c.lock().index.clone().unwrap())
            .collect();
        assert_eq!(
            names,
            vec![
                "users_email_unique",
                "users_account_id_created_at_index",
                "users_user_id_foreign"
            ]
        );

        let table = Blueprint::with_prefix("geo.users", "prefix_");
        assert_eq!(
            table.shared.create_index_name("index", &["a".into()]),
            "prefix_geo_users_a_index"
        );
    }

    #[test]
    fn constrained_guesses_the_table() {
        let mut table = Blueprint::new("posts");
        let foreign = table
            .foreign_id("user_id")
            .constrained()
            .cascade_on_delete();
        let attributes = foreign.attributes();
        assert_eq!(attributes.on.as_deref(), Some("users"));
        assert_eq!(attributes.references, vec!["id"]);
        assert_eq!(attributes.on_delete.as_deref(), Some("cascade"));
        assert_eq!(attributes.index.as_deref(), Some("posts_user_id_foreign"));

        let foreign = table.foreign_id("category_id").constrained();
        assert_eq!(foreign.attributes().on.as_deref(), Some("categories"));
    }

    #[test]
    fn columns_are_commands_when_altering() {
        let mut table = Blueprint::new("users");
        table.string("name");
        assert_eq!(table.get_commands().len(), 1);

        let mut table = Blueprint::creating("users");
        table.string("name");
        assert_eq!(table.get_commands().len(), 1);
        assert_eq!(table.get_added_columns().len(), 1);
    }
}
