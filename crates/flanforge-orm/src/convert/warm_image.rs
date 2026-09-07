use flanforge_core::WarmImageRecord;
use validator::Validate;

use super::{
    ConvertError,
    token::{from_json, from_token, to_i64, to_json, to_token, to_u64},
};
use crate::entities::warm_image::Model;

/// # Errors
///
/// Returns an error when a value does not fit its column.
pub fn warm_image_to_model(record: &WarmImageRecord) -> Result<Model, ConvertError> {
    Ok(Model {
        profile: to_token("profile", &record.profile)?,
        warm_template: to_token("warm_template", &record.warm_template)?,
        generation: to_i64("generation", record.generation)?,
        base_fingerprint: to_token("base_fingerprint", &record.base_fingerprint)?,
        produced_by: to_token("produced_by", &record.produced_by)?,
        produced_at_unix: to_i64("produced_at_unix", record.produced_at_unix)?,
        state: to_token("state", &record.state)?,
        previous: record
            .previous
            .as_ref()
            .map(|previous| to_json("previous", previous))
            .transpose()?,
    })
}

/// # Errors
///
/// Returns an error when the row cannot be mapped or does not validate.
pub fn warm_image_from_model(model: &Model) -> Result<WarmImageRecord, ConvertError> {
    let record = WarmImageRecord {
        profile: from_token("profile", &model.profile)?,
        warm_template: from_token("warm_template", &model.warm_template)?,
        generation: to_u64("generation", model.generation)?,
        base_fingerprint: from_token("base_fingerprint", &model.base_fingerprint)?,
        produced_by: from_token("produced_by", &model.produced_by)?,
        produced_at_unix: to_u64("produced_at_unix", model.produced_at_unix)?,
        state: from_token("state", &model.state)?,
        previous: model
            .previous
            .as_deref()
            .map(|previous| from_json("previous", previous))
            .transpose()?,
    };
    if record.validate().is_err() {
        return Err(ConvertError::Invalid {
            id: model.profile.clone(),
        });
    }
    Ok(record)
}
