
#![cfg_attr(not(feature = "speech"), allow(dead_code))]

pub mod config;
pub mod controller;
pub mod engine;
pub mod events;
pub mod model;
pub mod pipeline;

#[cfg(feature = "speech")]
mod sherpa;
#[cfg(feature = "speech")]
mod vad;

pub fn input_devices() -> Vec<String> {
    #[cfg(feature = "speech")]
    return crate::platform::audio::input_devices();
    #[cfg(not(feature = "speech"))]
    Vec::new()
}

pub const AVAILABLE: bool = cfg!(feature = "speech");
