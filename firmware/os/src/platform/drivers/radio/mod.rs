//! esp-radio adapter boundary. Services use owned requests/results so a future
//! HAL backend can replace this module without changing commands or storage.
pub mod classic;
pub mod https;
pub mod wifi;
