#[cfg(feature = "fail-injection")]
pub mod fail_injection;

#[cfg(test)]
#[cfg(feature = "fail-injection")]
mod fail_injection_tests;

#[cfg(test)]
#[cfg(feature = "real-db-tests")]
mod protocol_integration;

#[cfg(test)]
#[cfg(feature = "real-db-tests")]
mod reconcile_integration;

#[cfg(test)]
#[cfg(feature = "real-db-tests")]
mod manifest_validation_integration;

#[cfg(test)]
#[cfg(feature = "real-db-tests")]
mod m9_main_chain_integration;

#[cfg(test)]
#[cfg(all(feature = "real-db-tests", feature = "fail-injection"))]
mod cancellation_recovery_integration;
