//! External MySQL adoption: the AIP schema holds one updatable view per resource that renames the business table's
//! columns to the engine's names, exactly like the PostgreSQL adoption does. The business tables are never altered.
//! Views are `SQL SECURITY INVOKER`, so they cannot raise privileges: the runtime account needs the business table
//! grants itself.
use crate::mysql_catalog::CatalogError;
use serde_json::Value;
use spike_v2_read::external::Mapping;
use spike_v2_read::sqlgen;

fn quote(name: &str) -> String {
    format!("`{}`", name.replace('`', "``"))
}

fn valid(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && !name.contains('\0')
}

/// One `CREATE VIEW` per resource, in resource-name order. Every field of the resource must be mapped to a physical
/// column; a missing mapping is an error, never a silently dropped column.
pub fn view_statements(meta_schema: &str, facts: &Value, mapping: &Mapping) -> Result<Vec<String>, CatalogError> {
    if !valid(meta_schema) || !valid(&mapping.schema) || meta_schema == mapping.schema {
        return Err(CatalogError::new("AIP schema and business schema must be distinct valid identifiers"));
    }
    let resources = facts["resources"].as_object().ok_or_else(|| CatalogError::new("facts have no resources"))?;
    let mut statements = Vec::with_capacity(resources.len());
    for (resource, definition) in resources {
        let table = mapping.tables.get(resource).ok_or_else(|| CatalogError::new(format!("resource {resource} has no table mapping")))?;
        if !valid(&table.table) {
            return Err(CatalogError::new(format!("mapped table name for {resource} is invalid")));
        }
        let fields = definition["fields"].as_object().ok_or_else(|| CatalogError::new(format!("resource {resource} has no fields")))?;
        if table.columns.len() != fields.len() {
            return Err(CatalogError::new(format!("resource {resource} must map exactly its declared fields")));
        }
        let mut projection = Vec::with_capacity(fields.len());
        for field in fields.keys() {
            let physical = table
                .columns
                .get(field)
                .filter(|name| valid(name))
                .ok_or_else(|| CatalogError::new(format!("field {resource}.{field} has no valid column mapping")))?;
            let engine_column = sqlgen::column(facts, resource, field).ok_or_else(|| CatalogError::new("field type missing"))?;
            projection.push(format!("{} AS {}", quote(physical), quote(&engine_column)));
        }
        statements.push(format!(
            "CREATE OR REPLACE SQL SECURITY INVOKER VIEW {}.{} AS SELECT {} FROM {}.{}",
            quote(meta_schema),
            quote(&sqlgen::snake(resource)),
            projection.join(", "),
            quote(&mapping.schema),
            quote(&table.table),
        ));
    }
    Ok(statements)
}
