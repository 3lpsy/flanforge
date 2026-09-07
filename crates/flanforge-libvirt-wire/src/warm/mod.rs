mod limits;
mod model;
mod validate;

pub use limits::{
    MAX_PUBLISHED_WARM_BYTES, MAX_RETAINED_WARM_GENERATIONS, MAX_SUPERSEDED_POINTERS,
};
pub use model::PublishedWarm;

#[cfg(test)]
mod tests;
