use sea_orm::entity::prelude::*;

/// One row per profile's warm image; `previous` is the retained rollback
/// generation as a JSON document.
#[derive(Clone, Debug, DeriveEntityModel, Eq, PartialEq)]
#[sea_orm(table_name = "warm_images")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub profile: String,
    pub warm_template: String,
    pub generation: i64,
    pub base_fingerprint: String,
    pub produced_by: String,
    pub produced_at_unix: i64,
    pub state: String,
    pub previous: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
