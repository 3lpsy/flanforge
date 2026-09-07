mod allocations;
mod hot;
mod warm;

pub(crate) use allocations::cleanup_only_profile;

#[cfg(test)]
mod tests;
