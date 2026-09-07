use sea_orm::entity::prelude::*;

/// One row per retained hot machine. `claimed_by` is a plain column, not a
/// foreign key: allocation rows are prunable history.
#[derive(Clone, Debug, DeriveEntityModel, Eq, PartialEq)]
#[sea_orm(table_name = "hot_guests")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub vm_name: String,
    pub profile: String,
    pub lane: String,
    pub state: String,
    pub size_cpu_count: i64,
    pub size_memory_mb: i64,
    pub size_storage_mb: i64,
    pub source_name: String,
    pub source_kind: String,
    pub source_fingerprint: Option<String>,
    pub source_fallback_reason: Option<String>,
    pub warm_generation: Option<i64>,
    pub age_limit_seconds: Option<i64>,
    pub booted_at_unix: i64,
    pub updated_at_unix: i64,
    pub jobs_served: i64,
    pub claimed_by: Option<String>,
    pub drain_reason: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
