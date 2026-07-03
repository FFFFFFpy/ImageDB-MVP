#[cfg(feature = "fail-injection")]
pub mod fail_injection;

#[cfg(test)]
#[cfg(feature = "fail-injection")]
mod fail_injection_tests;
