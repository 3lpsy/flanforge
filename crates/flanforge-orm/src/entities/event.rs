use sea_orm::entity::prelude::*;

/// The durable "what has happened" log, bounded by retention.
#[derive(Clone, Debug, DeriveEntityModel, Eq, PartialEq)]
#[sea_orm(table_name = "events")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub occurred_at_unix: i64,
    pub kind: String,
    pub allocation_id: Option<String>,
    pub vm_name: Option<String>,
    pub profile: Option<String>,
    pub actor: Option<String>,
    pub payload: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
