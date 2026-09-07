use sea_orm::entity::prelude::*;

/// One row per allocation, terminal rows included; the queryable history.
/// Enum columns hold the domain types' serde tokens; `origin` and `retention`
/// are whole JSON documents.
#[derive(Clone, Debug, DeriveEntityModel, Eq, PartialEq)]
#[sea_orm(table_name = "allocations")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub profile: String,
    pub repository: String,
    pub run_id: i64,
    pub run_attempt: i64,
    pub state: String,
    pub mode: String,
    pub origin: String,
    pub vm_name: String,
    pub runner_label: String,
    pub vm_created: bool,
    pub runner_id: Option<i64>,
    pub created_at_unix: i64,
    pub updated_at_unix: i64,
    pub error: Option<String>,
    pub size_cpu_count: Option<i64>,
    pub size_memory_mb: Option<i64>,
    pub size_storage_mb: Option<i64>,
    pub source_name: Option<String>,
    pub source_kind: Option<String>,
    pub source_fingerprint: Option<String>,
    pub source_fallback_reason: Option<String>,
    pub hot_lane: Option<String>,
    pub hot_refusal: Option<String>,
    pub hot_age_seconds: Option<i64>,
    pub warm_generation: Option<i64>,
    pub retention: Option<String>,
    pub terminal_reason: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
