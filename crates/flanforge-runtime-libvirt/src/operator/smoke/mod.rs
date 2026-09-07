mod bind;
mod checks;
mod cleanup;
mod run;
mod state;

pub use run::smoke_disposable;

#[cfg(test)]
mod tests;
