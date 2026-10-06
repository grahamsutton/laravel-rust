//! The current state of a table, used by SQLite to rebuild tables for
//! changes it can't make in place (Laravel's `BlueprintState`).

use super::blueprint::{ColumnAttributes, ColumnDefault, CommandAttributes};
use super::{ColumnInfo, ForeignKeyInfo, IndexInfo};
use crate::expression::{Expression, Ident};

/// A table's columns, primary key, indexes and foreign keys, updated as a
/// blueprint's commands are compiled.
#[derive(Clone, Debug, Default)]
pub struct TableState {
    pub(crate) columns: Vec<ColumnAttributes>,
    pub(crate) primary: Option<CommandAttributes>,
    pub(crate) indexes: Vec<CommandAttributes>,
    pub(crate) original_indexes: Vec<CommandAttributes>,
    pub(crate) foreign_keys: Vec<CommandAttributes>,
    pub(crate) foreign_keys_enabled: bool,
}

impl TableState {
    /// Build the state from the table's introspected schema.
    pub(crate) fn new(
        columns: Vec<ColumnInfo>,
        indexes: Vec<IndexInfo>,
        foreign_keys: Vec<ForeignKeyInfo>,
        foreign_keys_enabled: bool,
    ) -> Self {
        let columns = columns
            .into_iter()
            .map(|column| ColumnAttributes {
                name: column.name,
                kind: column.type_name,
                full_type_definition: Some(column.type_),
                nullable: Some(column.nullable),
                default: column
                    .default
                    .map(|d| ColumnDefault::Raw(Expression::new(format!("({d})")))),
                auto_increment: column.auto_increment,
                collation: column.collation,
                comment: column.comment,
                ..Default::default()
            })
            .collect();

        let mut primary = None;
        let mut all_indexes = Vec::new();
        for index in indexes {
            let command = CommandAttributes {
                name: if index.primary {
                    "primary".into()
                } else if index.unique {
                    "unique".into()
                } else {
                    "index".into()
                },
                index: Some(index.name),
                columns: index.columns.into_iter().map(Ident::Name).collect(),
                ..Default::default()
            };
            if command.name == "primary" {
                primary = Some(command);
            } else {
                all_indexes.push(command);
            }
        }

        let foreign_keys = foreign_keys
            .into_iter()
            .map(|fk| CommandAttributes {
                name: "foreign".into(),
                columns: fk.columns.into_iter().map(Ident::Name).collect(),
                on: Some(fk.foreign_table),
                references: fk.foreign_columns,
                on_update: Some(fk.on_update).filter(|a| !a.is_empty() && a != "no action"),
                on_delete: Some(fk.on_delete).filter(|a| !a.is_empty() && a != "no action"),
                ..Default::default()
            })
            .collect();

        Self {
            columns,
            primary,
            original_indexes: all_indexes.clone(),
            indexes: all_indexes,
            foreign_keys,
            foreign_keys_enabled,
        }
    }

    /// Apply a blueprint command to the state.
    pub(crate) fn update(&mut self, command: &CommandAttributes) {
        match command.name.as_str() {
            "add" => {
                if let Some(column) = &command.column {
                    self.columns.push(column.attributes());
                }
            }
            "change" => {
                if let Some(column) = &command.column {
                    let column = column.attributes();
                    if let Some(existing) = self.columns.iter_mut().find(|c| c.name == column.name) {
                        *existing = column;
                    }
                }
            }
            "renameColumn" => {
                let from = command.from.clone().unwrap_or_default();
                let to = command.to.clone().unwrap_or_default();
                if let Some(column) = self.columns.iter_mut().find(|c| c.name == from) {
                    column.name = to.clone();
                }
                let rename = |columns: &mut Vec<Ident>| {
                    for column in columns.iter_mut() {
                        if column.value() == from {
                            *column = Ident::Name(to.clone());
                        }
                    }
                };
                if let Some(primary) = self.primary.as_mut() {
                    rename(&mut primary.columns);
                }
                for index in self.indexes.iter_mut() {
                    rename(&mut index.columns);
                }
                for foreign in self.foreign_keys.iter_mut() {
                    rename(&mut foreign.columns);
                }
            }
            "dropColumn" => {
                let names = command.column_names();
                self.columns.retain(|c| !names.contains(&c.name));
            }
            "primary" => self.primary = Some(command.clone()),
            "unique" | "index" => self.indexes.push(command.clone()),
            "renameIndex" => {
                if let Some(index) = self.indexes.iter_mut().find(|i| i.index == command.from) {
                    index.index = command.to.clone();
                }
            }
            "foreign" => self.foreign_keys.push(command.clone()),
            "dropPrimary" => self.primary = None,
            "dropIndex" | "dropUnique" => self.indexes.retain(|i| i.index != command.index),
            "dropForeign" => {
                let columns = command.column_names();
                self.foreign_keys.retain(|fk| fk.column_names() != columns);
            }
            _ => {}
        }
    }
}
